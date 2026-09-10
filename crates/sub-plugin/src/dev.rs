//! Dev installs and hot reload: the loop an agent runs while writing a plugin
//! (docs/PLAN.md §6.4).
//!
//! ```text
//! subordinate-cli plugin install ./target/wasm32-wasip2/release/my_plugin.wasm --dev
//! ```
//!
//! A **dev install** is an ordinary plugin directory in one of the two
//! [`InstallLocation`]s, except that its contents point back at the source
//! tree instead of being a snapshot of it: every top-level entry of the
//! directory holding `plugin.toml` is symlinked into it, and the built
//! component is linked in as [`WASM_FILE_NAME`]. A [`DEV_FILE_NAME`] beside
//! them records where those sources are, which is what makes the install
//! recognisable later — [`InstalledPlugin::dev`](crate::registry::InstalledPlugin::dev)
//! is that file's presence — and what the watcher reads.
//!
//! Symlinks are not available to every process on Windows, so a dev install
//! falls back to copying ([`LinkMode::Copy`]) and [`DevInstall::refresh`]
//! re-copies a changed source before each reload. Either way the *reload*
//! path is the same.
//!
//! # Watching
//!
//! [`DevWatcher`] polls the fingerprint — length and modification time — of
//! each dev plugin's sources rather than taking a dependency on a platform
//! notification API, so it behaves identically on all three OSes and in tests.
//! A change is reported only once its fingerprint has held still for one poll,
//! which is what keeps a half-written `.wasm` from being loaded: at the
//! default [`POLL_INTERVAL`] a rebuild is seen within 400 ms, comfortably
//! inside the second the developer loop asks for.
//!
//! # Reloading
//!
//! [`DevHost`] owns the swap. It holds the registry, a host-supplied loader —
//! the host is what knows how to instantiate a component and ask it for its
//! exports — and, per plugin, the [`PluginArtifacts`] that load contributed.
//! [`DevHost::reload`] runs the loader again and, **only if it succeeds**,
//! replaces that plugin's entries in the [`PluginCommandRegistry`], its
//! effect declarations and its [`ToolCatalog`]. Nothing else is touched: the
//! engine, the open project and the undo stack are not part of a reload, and a
//! reload that fails leaves the previous version registered and running.
//!
//! Every reload, good or bad, is recorded as a [`ReloadStatus`] — the row a
//! plugins panel shows (TASK-98) — and a failure is *also* returned as a
//! [`SubError`] with its stable code, so the CLI and an agent over MCP get the
//! structured error rather than a log line.
//!
//! ```
//! use std::sync::Arc;
//! use sub_plugin::dev::{DevHost, PluginArtifacts};
//! use sub_plugin::registry::{PluginDirs, PluginRegistry};
//!
//! let temp = std::env::temp_dir().join("subordinate-dev-doctest");
//! std::fs::remove_dir_all(&temp).ok();
//! let source = temp.join("src");
//! std::fs::create_dir_all(&source).unwrap();
//! std::fs::write(
//!     source.join("plugin.toml"),
//!     "[plugin]\nid = \"com.example.gain\"\nname = \"Gain\"\nversion = \"0.1.0\"\n\
//!      api = \"0.1\"\nworlds = [\"audio-effect\"]\n",
//! )
//! .unwrap();
//! std::fs::write(source.join("plugin.wasm"), b"\0asm\x01\0\0\0").unwrap();
//!
//! let registry = Arc::new(PluginRegistry::new(PluginDirs::new(temp.join("user"))));
//! let install = sub_plugin::dev::install(registry.dirs(), None, &source, true).unwrap();
//! assert!(install.dev);
//!
//! // The loader is the host's: here it contributes nothing.
//! let mut host = DevHost::new(Arc::clone(&registry), |_, _| Ok(PluginArtifacts::default()));
//! let status = host.reload(&install.id).unwrap();
//! assert!(status.ok);
//! assert_eq!(status.generation, 1);
//! std::fs::remove_dir_all(&temp).ok();
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sub_command::Dispatcher;
use sub_core::{SubError, SubResult};

use crate::WitEffectDesc;
use crate::codes;
use crate::manifest::{MANIFEST_FILE_NAME, Manifest, PluginId};
use crate::mcp::{ToolCatalog, ToolDeclaration};
use crate::menu::{CommandDesc, PluginCommandRegistry};
use crate::registry::{InstallLocation, InstalledPlugin, PluginDirs, PluginRegistry, Scan};
use crate::runtime::PluginRuntime;

/// The file that records a dev install's sources, written in the installed
/// directory. Its presence is what makes an install a dev install.
pub const DEV_FILE_NAME: &str = "dev.json";

/// The version stamped into that file.
pub const DEV_SCHEMA_VERSION: u32 = 1;

/// The name the built component is installed under, whatever it was called in
/// the build directory.
pub const WASM_FILE_NAME: &str = "plugin.wasm";

/// How often [`DevWatcher::spawn`] looks at a dev plugin's sources.
pub const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// How far up from a `.wasm` a manifest is looked for.
const MANIFEST_SEARCH_DEPTH: usize = 8;

/// How a dev install's files point at the source tree.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum LinkMode {
    /// Each file is a symlink, so a rebuild is visible with no further work.
    Symlink,
    /// Each file is a copy, because this platform refused the symlink. The
    /// copies are refreshed before every reload.
    Copy,
}

impl LinkMode {
    /// The name used in JSON and on the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Symlink => "symlink",
            Self::Copy => "copy",
        }
    }
}

impl std::fmt::Display for LinkMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a dev install's files came from.
///
/// `root` is the directory holding `plugin.toml`; `wasm` is the built
/// component, which normally sits in a build directory well outside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevSource {
    /// The directory holding `plugin.toml`.
    #[schemars(with = "String")]
    pub root: PathBuf,
    /// The built component.
    #[schemars(with = "String")]
    pub wasm: PathBuf,
}

impl DevSource {
    /// Resolves what a user pointed `plugin install` at.
    ///
    /// `path` is either the built `.wasm` — the manifest is then looked for in
    /// its directory and up to [`MANIFEST_SEARCH_DEPTH`] parents above it, so
    /// `target/wasm32-wasip2/release/my_plugin.wasm` finds the crate's own
    /// `plugin.toml` — or the plugin directory itself, which must hold both a
    /// `plugin.toml` and exactly one `.wasm`.
    ///
    /// # Errors
    ///
    /// [`codes::DEV_SOURCE_INVALID`] when the path does not exist, holds no
    /// manifest, or holds no single component to install.
    pub fn resolve(path: &Path) -> SubResult<Self> {
        let invalid = |message: &str| {
            SubError::new(codes::DEV_SOURCE_INVALID, message)
                .with_detail("path", path.display().to_string())
        };
        let path = std::fs::canonicalize(path)
            .map_err(|err| invalid("the install source does not exist").with_cause(&err))?;

        if path.is_dir() {
            if !path.join(MANIFEST_FILE_NAME).is_file() {
                return Err(invalid("the install source holds no plugin.toml"));
            }
            let wasm = single_wasm(&path)?;
            return Ok(Self { root: path, wasm });
        }

        if path.extension().is_none_or(|ext| ext != "wasm") {
            return Err(invalid(
                "an install source is a .wasm file or a plugin directory",
            ));
        }
        let mut dir = path.parent().map(Path::to_path_buf);
        for _ in 0..=MANIFEST_SEARCH_DEPTH {
            let Some(candidate) = dir else { break };
            if candidate.join(MANIFEST_FILE_NAME).is_file() {
                return Ok(Self {
                    root: candidate,
                    wasm: path,
                });
            }
            dir = candidate.parent().map(Path::to_path_buf);
        }
        Err(invalid(
            "no plugin.toml was found beside the component or above it",
        ))
    }
}

/// The one `.wasm` in a plugin directory: [`WASM_FILE_NAME`] when it is there,
/// otherwise the single one present.
fn single_wasm(dir: &Path) -> SubResult<PathBuf> {
    let named = dir.join(WASM_FILE_NAME);
    if named.is_file() {
        return Ok(named);
    }
    let entries = std::fs::read_dir(dir).map_err(|err| {
        SubError::wrap(
            codes::DEV_SOURCE_INVALID,
            "the install source could not be listed",
            &err,
        )
        .with_detail("path", dir.display().to_string())
    })?;
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "wasm") {
            found.push(path);
        }
    }
    found.sort();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(SubError::new(
            codes::DEV_SOURCE_INVALID,
            "the install source holds no .wasm",
        )
        .with_detail("path", dir.display().to_string())),
        count => Err(SubError::new(
            codes::DEV_SOURCE_INVALID,
            "the install source holds more than one .wasm; name the one to install",
        )
        .with_detail("path", dir.display().to_string())
        .with_detail("count", count)),
    }
}

/// The `dev.json` an install writes, as it is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DevRecord {
    /// The version of this file's shape.
    schema_version: u32,
    /// The plugin this directory holds, so a stray file is recognisable.
    id: PluginId,
    /// Where its files came from.
    source: DevSource,
    /// How they were linked in.
    mode: LinkMode,
}

impl DevRecord {
    /// Reads the record in `directory`, or `None` when it holds none — which
    /// is what an ordinary install looks like.
    fn read(directory: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(directory.join(DEV_FILE_NAME)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Writes the record into `directory`.
    fn write(&self, directory: &Path) -> SubResult<()> {
        let path = directory.join(DEV_FILE_NAME);
        let mut text = serde_json::to_string_pretty(self).map_err(|err| {
            SubError::wrap(
                codes::INSTALL_FAILED,
                "the dev install record could not be serialised",
                &err,
            )
        })?;
        text.push('\n');
        std::fs::write(&path, text).map_err(|err| install_failed(&path, &err))
    }
}

/// What an install did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevInstall {
    /// The plugin that was installed.
    pub id: PluginId,
    /// Which of the two directories it went into.
    pub location: InstallLocation,
    /// The directory it now occupies.
    #[schemars(with = "String")]
    pub directory: PathBuf,
    /// Whether it is a dev install: linked to its sources and watched.
    pub dev: bool,
    /// How its files point at the source tree. A plain install is a
    /// [`LinkMode::Copy`] snapshot that is never refreshed.
    pub mode: LinkMode,
    /// Where its files came from.
    pub source: DevSource,
}

impl DevInstall {
    /// The paths a watcher looks at for this install: the source manifest and
    /// the built component.
    #[must_use]
    pub fn watched(&self) -> Vec<PathBuf> {
        vec![
            self.source.root.join(MANIFEST_FILE_NAME),
            self.source.wasm.clone(),
        ]
    }

    /// Brings the installed directory back in step with its sources.
    ///
    /// A no-op for a symlinked install, which is always in step; a re-copy for
    /// one that fell back to copies. Called before every reload.
    ///
    /// # Errors
    ///
    /// [`codes::INSTALL_FAILED`] when a source cannot be re-copied, and
    /// [`codes::DEV_SOURCE_INVALID`] when it is no longer there at all.
    pub fn refresh(&self) -> SubResult<()> {
        if self.mode == LinkMode::Symlink || !self.dev {
            return Ok(());
        }
        materialise(&self.source, &self.directory, LinkMode::Copy).map(|_| ())
    }

    /// The dev install recorded in `directory`, or `None` when it is an
    /// ordinary one.
    #[must_use]
    pub fn read(directory: &Path, location: InstallLocation) -> Option<Self> {
        let record = DevRecord::read(directory)?;
        (record.schema_version == DEV_SCHEMA_VERSION).then(|| Self {
            id: record.id,
            location,
            directory: directory.to_path_buf(),
            dev: true,
            mode: record.mode,
            source: record.source,
        })
    }

    /// The dev install behind an installed plugin, or `None` when it is an
    /// ordinary one.
    #[must_use]
    pub fn of(plugin: &InstalledPlugin) -> Option<Self> {
        Self::read(&plugin.directory, plugin.location)
    }
}

/// Installs a plugin into one of the plugin directories.
///
/// `source` is what [`DevSource::resolve`] accepts: the built `.wasm` or the
/// directory holding `plugin.toml`. `location` defaults to
/// [`InstallLocation::User`]; the project-local one is only available while a
/// project is open.
///
/// With `dev` the installed directory is linked to the source tree and a
/// [`DEV_FILE_NAME`] records where it points, so [`DevWatcher`] can watch it
/// and [`DevHost::reload`] can pick a rebuild up. Without it the source is
/// copied once and nothing watches anything.
///
/// Installing over an existing copy of the *same* plugin replaces it; a
/// directory already holding a different plugin is refused rather than
/// overwritten.
///
/// # Errors
///
/// [`codes::DEV_SOURCE_INVALID`] for a source that is not a plugin,
/// [`codes::INVALID_MANIFEST`] and friends for a `plugin.toml` that does not
/// parse, [`codes::PLUGIN_DIR_UNAVAILABLE`] when the chosen location has no
/// directory, [`codes::INSTALL_CONFLICT`] when another plugin already occupies
/// the target directory and [`codes::INSTALL_FAILED`] when the files cannot be
/// written.
pub fn install(
    dirs: &PluginDirs,
    location: Option<InstallLocation>,
    source: &Path,
    dev: bool,
) -> SubResult<DevInstall> {
    crate::errors::explained(install_inner(dirs, location, source, dev))
}

/// The install proper; [`install`] is this plus the error catalogue, so every
/// failure names the hint its code implies.
fn install_inner(
    dirs: &PluginDirs,
    location: Option<InstallLocation>,
    source: &Path,
    dev: bool,
) -> SubResult<DevInstall> {
    let location = location.unwrap_or(InstallLocation::User);
    let source = DevSource::resolve(source)?;
    let manifest = Manifest::read_dir(&source.root)?;
    let id = manifest.plugin.id.clone();

    let base = dirs.dir(location).ok_or_else(|| {
        SubError::new(
            codes::PLUGIN_DIR_UNAVAILABLE,
            "there is no project-local plugin directory, because no project is open",
        )
        .with_detail("location", location.as_str())
    })?;
    let directory = base.join(id.as_str());

    if directory.exists() {
        let occupant = Manifest::read_dir(&directory)
            .ok()
            .map(|existing| existing.plugin.id);
        if occupant.is_some_and(|existing| existing != id) {
            return Err(SubError::new(
                codes::INSTALL_CONFLICT,
                "another plugin already occupies that directory; remove it first",
            )
            .with_detail("id", id.as_str())
            .with_detail("path", directory.display().to_string()));
        }
        std::fs::remove_dir_all(&directory).map_err(|err| install_failed(&directory, &err))?;
    }
    std::fs::create_dir_all(&directory).map_err(|err| install_failed(&directory, &err))?;

    let mode = materialise(
        &source,
        &directory,
        if dev {
            LinkMode::Symlink
        } else {
            LinkMode::Copy
        },
    )?;
    let install = DevInstall {
        id: id.clone(),
        location,
        directory,
        dev,
        mode,
        source,
    };
    if dev {
        DevRecord {
            schema_version: DEV_SCHEMA_VERSION,
            id,
            source: install.source.clone(),
            mode,
        }
        .write(&install.directory)?;
    }
    Ok(install)
}

/// Puts every top-level entry of `source.root` plus the built component into
/// `directory`, and answers the mode that actually worked.
///
/// `wanted` is only a preference: a platform that refuses symlinks — an
/// unprivileged Windows process — falls back to copying, and the caller
/// records which happened so a reload knows whether it must re-copy.
fn materialise(source: &DevSource, directory: &Path, wanted: LinkMode) -> SubResult<LinkMode> {
    let mut mode = wanted;
    let entries = std::fs::read_dir(&source.root).map_err(|err| {
        SubError::wrap(
            codes::DEV_SOURCE_INVALID,
            "the plugin source directory could not be listed",
            &err,
        )
        .with_detail("path", source.root.display().to_string())
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| install_failed(&source.root, &err))?;
        let name = entry.file_name();
        // The built component is installed from its own path, under the one
        // name the host loads it by, so a stale copy in the source tree never
        // wins.
        if name == WASM_FILE_NAME || name == DEV_FILE_NAME {
            continue;
        }
        mode = mode.min(place(&entry.path(), &directory.join(&name), mode)?);
    }
    mode = mode.min(place(&source.wasm, &directory.join(WASM_FILE_NAME), mode)?);
    Ok(mode)
}

impl LinkMode {
    /// The weaker of two modes: one entry that had to be copied makes the
    /// whole install a copy, because the install is only as live as its least
    /// live file.
    fn min(self, other: Self) -> Self {
        if self == Self::Copy || other == Self::Copy {
            Self::Copy
        } else {
            Self::Symlink
        }
    }
}

/// Puts one source entry at `target`, symlinking it when asked and possible.
fn place(source: &Path, target: &Path, mode: LinkMode) -> SubResult<LinkMode> {
    if target.exists() || target.symlink_metadata().is_ok() {
        remove(target)?;
    }
    if mode == LinkMode::Symlink && symlink(source, target).is_ok() {
        return Ok(LinkMode::Symlink);
    }
    if source.is_dir() {
        copy_dir(source, target)?;
    } else {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|err| install_failed(parent, &err))?;
        }
        std::fs::copy(source, target).map_err(|err| install_failed(target, &err))?;
    }
    Ok(LinkMode::Copy)
}

/// Deletes whatever is at `path`, link or directory.
fn remove(path: &Path) -> SubResult<()> {
    let meta = std::fs::symlink_metadata(path).map_err(|err| install_failed(path, &err))?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map_err(|err| install_failed(path, &err))
}

/// A symlink at `target` pointing at `source`, on whichever platform this is.
#[cfg(unix)]
fn symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

/// A symlink at `target` pointing at `source`, on whichever platform this is.
///
/// Windows needs the right kind for the kind of thing pointed at, and refuses
/// both to a process without the privilege — which is why the caller falls
/// back to copying.
#[cfg(windows)]
fn symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::os::windows::fs::symlink_dir(source, target)
    } else {
        std::os::windows::fs::symlink_file(source, target)
    }
}

/// Copies a directory tree.
fn copy_dir(source: &Path, target: &Path) -> SubResult<()> {
    std::fs::create_dir_all(target).map_err(|err| install_failed(target, &err))?;
    let entries = std::fs::read_dir(source).map_err(|err| install_failed(source, &err))?;
    for entry in entries {
        let entry = entry.map_err(|err| install_failed(source, &err))?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|err| install_failed(&to, &err))?;
        }
    }
    Ok(())
}

/// The error an install answers when the filesystem refuses it.
fn install_failed(path: &Path, err: &std::io::Error) -> SubError {
    SubError::wrap(
        codes::INSTALL_FAILED,
        "the plugin could not be installed",
        err,
    )
    .with_detail("path", path.display().to_string())
}

/// What one watched file looked like when it was last seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    /// Its length in bytes.
    len: u64,
    /// Its modification time, when the platform reports one.
    modified: Option<SystemTime>,
}

impl Fingerprint {
    /// The fingerprint of `path`, or `None` when it is not there — a file that
    /// comes and goes during a rebuild is a change like any other.
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// What each watched plugin looked like when it was last seen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Seen {
    /// The fingerprint that was last *reported*.
    reported: Vec<Option<Fingerprint>>,
    /// The fingerprint seen on the previous poll, which has to match the
    /// current one before a change is reported.
    pending: Vec<Option<Fingerprint>>,
}

/// A poll-based watcher over the sources of dev-installed plugins.
///
/// It takes no dependency on a platform notification API: the file counts here
/// are tiny — one manifest and one component per plugin — and polling behaves
/// the same on every OS and in a test. A change is reported only after its
/// fingerprint has held still for one further poll, so a `.wasm` that is
/// halfway through being written is never handed to the loader.
#[derive(Debug, Default)]
pub struct DevWatcher {
    /// The files watched for each plugin.
    targets: BTreeMap<PluginId, Vec<PathBuf>>,
    /// What those files last looked like.
    seen: BTreeMap<PluginId, Seen>,
}

impl DevWatcher {
    /// A watcher over nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A watcher over every dev install in `scan`.
    #[must_use]
    pub fn from_scan(scan: &Scan) -> Self {
        let mut watcher = Self::new();
        watcher.sync(scan);
        watcher
    }

    /// Brings the watched set in line with `scan`: newly dev-installed plugins
    /// start being watched, and ones that are gone or no longer dev installs
    /// are forgotten.
    ///
    /// A plugin taken up here is watched with **no** baseline, so its first
    /// settled reading is reported as a change: a host that syncs and reloads
    /// therefore loads every dev plugin once at startup, and cannot lose a
    /// rebuild that lands in the moment between the install and the first
    /// look. A plugin already watched keeps its baseline, so syncing does not
    /// lose a change that is halfway through settling.
    pub fn sync(&mut self, scan: &Scan) {
        let mut present = BTreeMap::new();
        for plugin in scan.plugins() {
            if let Some(install) = DevInstall::of(plugin) {
                present.insert(plugin.id.clone(), install.watched());
            }
        }
        let gone: Vec<PluginId> = self
            .targets
            .keys()
            .filter(|id| !present.contains_key(*id))
            .cloned()
            .collect();
        for id in gone {
            self.forget(&id);
        }
        for (id, paths) in present {
            if !self.targets.contains_key(&id) {
                self.watch_unseen(id, paths);
            }
        }
    }

    /// Watches `paths` on behalf of `id`, replacing anything watched for it
    /// before.
    ///
    /// The first [`DevWatcher::poll`] after this reports nothing: what is on
    /// disk now is the baseline.
    pub fn watch(&mut self, id: PluginId, paths: Vec<PathBuf>) {
        let current: Vec<Option<Fingerprint>> =
            paths.iter().map(|path| Fingerprint::of(path)).collect();
        self.seen.insert(
            id.clone(),
            Seen {
                reported: current.clone(),
                pending: current,
            },
        );
        self.targets.insert(id, paths);
    }

    /// Watches `paths` on behalf of `id` with nothing taken as already seen.
    ///
    /// The next poll to find them unchanged from the one before reports a
    /// change, which is how a host loads what is installed when it starts and
    /// how it avoids missing a rebuild that landed while it was starting.
    pub fn watch_unseen(&mut self, id: PluginId, paths: Vec<PathBuf>) {
        let pending: Vec<Option<Fingerprint>> =
            paths.iter().map(|path| Fingerprint::of(path)).collect();
        self.seen.insert(
            id.clone(),
            Seen {
                // No reading can equal an empty one, so the first settled look
                // is a change.
                reported: Vec::new(),
                pending,
            },
        );
        self.targets.insert(id, paths);
    }

    /// Stops watching a plugin.
    pub fn forget(&mut self, id: &PluginId) {
        self.targets.remove(id);
        self.seen.remove(id);
    }

    /// The plugins being watched.
    #[must_use]
    pub fn watched(&self) -> Vec<&PluginId> {
        self.targets.keys().collect()
    }

    /// Looks at every watched file once and answers the plugins whose sources
    /// have settled at something new.
    pub fn poll(&mut self) -> Vec<PluginId> {
        let mut changed = Vec::new();
        for (id, paths) in &self.targets {
            let current: Vec<Option<Fingerprint>> =
                paths.iter().map(|path| Fingerprint::of(path)).collect();
            let seen = self.seen.entry(id.clone()).or_default();
            if current != seen.reported && current == seen.pending {
                seen.reported.clone_from(&current);
                changed.push(id.clone());
            }
            seen.pending = current;
        }
        changed
    }

    /// Runs the watcher on its own thread, handing every change to `on_change`
    /// until the returned handle is dropped or stopped.
    ///
    /// `interval` is how often the files are looked at; a change is reported
    /// one interval after it settles, so [`POLL_INTERVAL`] sees a rebuild
    /// within 400 ms.
    #[must_use]
    pub fn spawn(
        mut self,
        interval: Duration,
        mut on_change: impl FnMut(&PluginId) + Send + 'static,
    ) -> WatchHandle {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("sub-plugin-dev-watch".to_owned())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    for id in self.poll() {
                        on_change(&id);
                    }
                    std::thread::sleep(interval);
                }
            })
            .ok();
        WatchHandle { stop, join }
    }
}

/// A running [`DevWatcher`]. Dropping it stops the thread.
#[derive(Debug)]
pub struct WatchHandle {
    /// Set to ask the thread to finish its current pass and stop.
    stop: Arc<AtomicBool>,
    /// The thread, unless it could not be started.
    join: Option<std::thread::JoinHandle<()>>,
}

impl WatchHandle {
    /// Stops the watcher and waits for its thread.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            join.join().ok();
        }
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Watches every dev install `host` knows about and reloads one as soon as its
/// sources settle at something new.
///
/// This is the developer loop's other half: `plugin install --dev` links the
/// build output in, and this is what notices the next `cargo build` and swaps
/// the plugin over — commands, effects and tools re-registered, engine and
/// project untouched. A rebuild that does not load leaves the running version
/// in place; the failure is recorded as that plugin's [`ReloadStatus`] and
/// logged, because there is no caller to answer here.
///
/// The set of watched plugins is re-derived from the registry on every pass,
/// so a plugin dev-installed while this runs is picked up without restarting
/// anything. Dropping the handle stops the thread.
#[must_use]
pub fn watch_and_reload(host: Arc<Mutex<DevHost>>, interval: Duration) -> WatchHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let join = std::thread::Builder::new()
        .name("sub-plugin-dev-reload".to_owned())
        .spawn(move || {
            let mut watcher = DevWatcher::new();
            while !flag.load(Ordering::Relaxed) {
                let scanned = match host.lock() {
                    Ok(guard) => guard.registry().scan(),
                    Err(_) => break,
                };
                match scanned {
                    Ok(scan) => watcher.sync(&scan),
                    Err(error) => tracing::warn!(
                        code = error.code.as_str(),
                        "the plugin directories could not be walked: {}",
                        error.message,
                    ),
                }
                for id in watcher.poll() {
                    if let Ok(mut guard) = host.lock() {
                        // A failure is already recorded and logged against the
                        // plugin; the watcher keeps going either way.
                        drop(guard.reload(&id));
                    }
                }
                std::thread::sleep(interval);
            }
        })
        .ok();
    WatchHandle { stop, join }
}

/// What one load of a plugin contributed.
///
/// The loader fills this in; [`DevHost`] is what registers it and, on the next
/// reload, replaces it.
#[derive(Debug, Clone, Default)]
pub struct PluginArtifacts {
    /// The menu and shortcut entries of the `commands` world.
    pub commands: Vec<CommandDesc>,
    /// The declarations of the `effect` world, one per exported effect.
    pub effects: Vec<WitEffectDesc>,
    /// The tools of the `mcp-tools` world, already validated against the
    /// manifest.
    pub tools: Vec<ToolDeclaration>,
}

/// How a host loads a plugin: given the installed plugin and the path of its
/// component, instantiate it and answer what it contributes.
///
/// The host owns this because instantiating a component needs the host's own
/// state — the Command API handle, the resolved capabilities — which this
/// module knows nothing about. [`compile_only_loader`] is the trivial one: it
/// proves the component compiles and contributes nothing.
pub type Loader = Arc<dyn Fn(&InstalledPlugin, &Path) -> SubResult<PluginArtifacts> + Send + Sync>;

/// A loader that compiles the component and contributes nothing.
///
/// It is what a process that manages plugins without running them uses —
/// `subordinate-cli plugin reload`, the headless `serve` — so a reload still
/// reports a component that will not load, with the same stable code the
/// editor would report.
#[must_use]
pub fn compile_only_loader(runtime: PluginRuntime) -> Loader {
    Arc::new(move |plugin: &InstalledPlugin, path: &Path| {
        runtime.compile_file(&plugin.id, path)?;
        Ok(PluginArtifacts::default())
    })
}

/// One error, flattened the way [`crate::registry::LoadFailure`] flattens one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReloadError {
    /// The stable error code, e.g. `plugin.load_failed`.
    pub code: String,
    /// The one-line message.
    pub message: String,
    /// The error's details.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(with = "BTreeMap<String, Value>")]
    pub details: BTreeMap<String, Value>,
}

impl From<&SubError> for ReloadError {
    /// Flattens a failed load, filling in the `wit` and `hint` details its
    /// code implies (see [`crate::errors`]) so the row a plugins panel or an
    /// agent reads says what to do about it.
    fn from(error: &SubError) -> Self {
        let error = crate::errors::explain(error.clone());
        Self {
            code: error.code.as_str().to_owned(),
            message: error.message,
            details: error.details,
        }
    }
}

/// What the last load of one plugin did.
///
/// This is the row a plugins panel shows (TASK-98) and what `plugin.reload`
/// answers. A failed reload keeps the counts of the version that is still
/// loaded, because that is the version still answering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReloadStatus {
    /// The plugin.
    pub id: PluginId,
    /// How many times it has loaded successfully in this process. A dev
    /// install's reloads are how a developer knows the rebuild landed.
    pub generation: u64,
    /// Whether the last attempt succeeded.
    pub ok: bool,
    /// Whether it is a dev install.
    pub dev: bool,
    /// How many menu commands the loaded version registers.
    pub commands: usize,
    /// How many effects it declares.
    pub effects: usize,
    /// How many MCP tools it contributes.
    pub tools: usize,
    /// Why the last attempt failed, when it did. The counts above then still
    /// describe the previous version, which stayed loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ReloadError>,
}

/// One loaded plugin's contributions.
#[derive(Debug)]
struct Loaded {
    /// What the last successful load contributed.
    artifacts: PluginArtifacts,
    /// Its tools, compiled into the catalogue an MCP bridge publishes.
    tools: ToolCatalog,
    /// How many successful loads there have been.
    generation: u64,
}

/// The host side of hot reload: what is loaded, and the swap when it changes.
///
/// It owns the [`PluginCommandRegistry`] a menu reads, the effect declarations
/// a compositor reads and one [`ToolCatalog`] per plugin, and it is the only
/// thing that replaces them. A reload never touches the engine, the project or
/// the undo stack, and a reload that fails changes nothing at all.
pub struct DevHost {
    /// Where plugins are installed.
    registry: Arc<PluginRegistry>,
    /// How a component becomes artifacts.
    loader: Loader,
    /// The commands every loaded plugin contributes.
    commands: PluginCommandRegistry,
    /// What each loaded plugin contributed.
    loaded: BTreeMap<PluginId, Loaded>,
    /// What each plugin's last load did, successful or not.
    statuses: BTreeMap<PluginId, ReloadStatus>,
}

impl std::fmt::Debug for DevHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevHost")
            .field("loaded", &self.loaded.keys().collect::<Vec<_>>())
            .field("commands", &self.commands.len())
            .finish_non_exhaustive()
    }
}

impl DevHost {
    /// A host over `registry` that loads plugins with `loader`.
    #[must_use]
    pub fn new(
        registry: Arc<PluginRegistry>,
        loader: impl Fn(&InstalledPlugin, &Path) -> SubResult<PluginArtifacts> + Send + Sync + 'static,
    ) -> Self {
        Self::with_loader(registry, Arc::new(loader))
    }

    /// A host over `registry` that loads plugins with an existing [`Loader`].
    #[must_use]
    pub fn with_loader(registry: Arc<PluginRegistry>, loader: Loader) -> Self {
        Self {
            registry,
            loader,
            commands: PluginCommandRegistry::new(),
            loaded: BTreeMap::new(),
            statuses: BTreeMap::new(),
        }
    }

    /// The registry it installs into and scans.
    #[must_use]
    pub fn registry(&self) -> &Arc<PluginRegistry> {
        &self.registry
    }

    /// The commands every loaded plugin contributes, which is what the Plugins
    /// menu and the keyboard map read.
    #[must_use]
    pub fn commands(&self) -> &PluginCommandRegistry {
        &self.commands
    }

    /// The effects one loaded plugin declares.
    #[must_use]
    pub fn effects(&self, id: &PluginId) -> &[WitEffectDesc] {
        self.loaded
            .get(id)
            .map_or(&[][..], |loaded| &loaded.artifacts.effects)
    }

    /// The MCP tools one loaded plugin contributes.
    #[must_use]
    pub fn tools(&self, id: &PluginId) -> Option<&ToolCatalog> {
        self.loaded.get(id).map(|loaded| &loaded.tools)
    }

    /// What each plugin's last load did, in id order. The rows a plugins panel
    /// shows.
    #[must_use]
    pub fn statuses(&self) -> Vec<ReloadStatus> {
        self.statuses.values().cloned().collect()
    }

    /// What one plugin's last load did.
    #[must_use]
    pub fn status(&self, id: &PluginId) -> Option<&ReloadStatus> {
        self.statuses.get(id)
    }

    /// Installs a plugin, then loads it.
    ///
    /// A dev install that loads is watchable straight away: the returned
    /// [`DevInstall::watched`] paths are what [`DevWatcher::watch`] takes.
    ///
    /// # Errors
    ///
    /// Whatever [`install`] and [`DevHost::reload`] return. A plugin that
    /// installs but does not load is left installed, with the failure recorded
    /// against it, because deleting a developer's just-built plugin because it
    /// does not compile would be the wrong answer.
    pub fn install(
        &mut self,
        location: Option<InstallLocation>,
        source: &Path,
        dev: bool,
    ) -> SubResult<DevInstall> {
        let install = install(self.registry.dirs(), location, source, dev)?;
        self.reload(&install.id)?;
        Ok(install)
    }

    /// Loads a plugin again, replacing what the previous load registered.
    ///
    /// The sequence is: re-scan so a manifest edit is picked up, refresh a
    /// copied dev install from its sources, run the loader, and only then
    /// swap the registrations. A failure at any step leaves the previously
    /// loaded version registered and running, records the failure as this
    /// plugin's [`ReloadStatus`], and returns it.
    ///
    /// # Errors
    ///
    /// [`codes::NOT_INSTALLED`] when nothing is installed under `id`,
    /// [`codes::LOAD_FAILED`] and the rest of the host's codes from the
    /// loader, and whatever [`PluginRegistry::scan`] returns. Each carries the
    /// WIT item it belongs to and a hint (see [`crate::errors`]), so the
    /// developer loop reports a bad build in terms of what to do about it.
    pub fn reload(&mut self, id: &PluginId) -> SubResult<ReloadStatus> {
        match self.try_reload(id) {
            Ok(status) => Ok(status),
            Err(error) => {
                let error = crate::errors::explain(error);
                self.record_failure(id, &error);
                Err(error)
            }
        }
    }

    /// Forgets a plugin: its commands, effects and tools are unregistered.
    ///
    /// Answers whether anything was loaded under that id.
    pub fn unload(&mut self, id: &PluginId) -> bool {
        self.commands.remove_plugin(id.as_str());
        self.statuses.remove(id);
        self.loaded.remove(id).is_some()
    }

    /// The reload proper, before a failure is recorded.
    fn try_reload(&mut self, id: &PluginId) -> SubResult<ReloadStatus> {
        let scan = self.registry.scan()?;
        let plugin = scan
            .get(id)
            .ok_or_else(|| {
                SubError::new(codes::NOT_INSTALLED, "no plugin is installed under that id")
                    .with_detail("id", id.as_str())
            })?
            .clone();
        let dev = DevInstall::of(&plugin);
        if let Some(install) = &dev {
            install.refresh()?;
        }

        let wasm = plugin.directory.join(WASM_FILE_NAME);
        if !wasm.is_file() {
            return Err(SubError::new(
                codes::LOAD_FAILED,
                "the installed plugin holds no plugin.wasm",
            )
            .with_detail("id", id.as_str())
            .with_detail("path", wasm.display().to_string()));
        }

        let artifacts = (self.loader)(&plugin, &wasm)?;
        // Everything that can be refused is checked before anything is
        // replaced, so a bad reload cannot leave half the old version
        // registered.
        let tools = ToolCatalog::new(id.as_str(), artifacts.tools.clone())?;
        let mut commands = self.commands.clone();
        commands.remove_plugin(id.as_str());
        commands.register(id.as_str(), &artifacts.commands)?;

        let generation = self
            .loaded
            .get(id)
            .map_or(0, |loaded| loaded.generation)
            .saturating_add(1);
        let status = ReloadStatus {
            id: id.clone(),
            generation,
            ok: true,
            dev: dev.is_some(),
            commands: artifacts.commands.len(),
            effects: artifacts.effects.len(),
            tools: artifacts.tools.len(),
            error: None,
        };
        self.commands = commands;
        self.loaded.insert(
            id.clone(),
            Loaded {
                artifacts,
                tools,
                generation,
            },
        );
        self.statuses.insert(id.clone(), status.clone());
        tracing::info!(
            plugin = id.as_str(),
            generation,
            commands = status.commands,
            effects = status.effects,
            tools = status.tools,
            "plugin loaded",
        );
        Ok(status)
    }

    /// Records a failed reload against `id` without disturbing what is loaded.
    fn record_failure(&mut self, id: &PluginId, error: &SubError) {
        tracing::warn!(
            plugin = id.as_str(),
            code = error.code.as_str(),
            "a plugin could not be reloaded: {}",
            error.message,
        );
        let previous = self.loaded.get(id);
        let status = ReloadStatus {
            id: id.clone(),
            generation: previous.map_or(0, |loaded| loaded.generation),
            ok: false,
            dev: self.statuses.get(id).is_some_and(|status| status.dev),
            commands: previous.map_or(0, |loaded| loaded.artifacts.commands.len()),
            effects: previous.map_or(0, |loaded| loaded.artifacts.effects.len()),
            tools: previous.map_or(0, |loaded| loaded.artifacts.tools.len()),
            error: Some(ReloadError::from(error)),
        };
        self.statuses.insert(id.clone(), status);
    }
}

/// `plugin.install`: install a plugin from a built component, optionally as a
/// dev install.
pub const PLUGIN_INSTALL: &str = "plugin.install";
/// `plugin.reload`: load an installed plugin again.
pub const PLUGIN_RELOAD: &str = "plugin.reload";
/// `plugin.status`: what each plugin's last load did.
pub const PLUGIN_STATUS: &str = "plugin.status";

/// The parameters of `plugin.install`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallParams {
    /// The built `.wasm`, or the directory holding `plugin.toml`.
    #[schemars(with = "String")]
    pub path: PathBuf,
    /// Link the install to its sources and watch them, instead of copying
    /// once.
    #[serde(default)]
    pub dev: bool,
    /// Which plugin directory to install into. Defaults to the per-user one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<InstallLocation>,
}

/// The parameters of `plugin.reload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReloadParams {
    /// The plugin's reverse-DNS id.
    pub id: PluginId,
}

/// The parameters of `plugin.status`: none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusParams {}

/// The result of `plugin.status`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Statuses {
    /// One row per plugin that has been loaded in this process, in id order.
    pub plugins: Vec<ReloadStatus>,
}

/// Puts `plugin.install`, `plugin.reload` and `plugin.status` on a
/// [`Dispatcher`], served by `host`.
///
/// They are queries like the rest of the plugin management surface: installing
/// or reloading a plugin changes what the host runs, never the project, so
/// nothing goes on the undo stack. A reload that fails answers the loader's
/// [`SubError`] — code, message and details — so the CLI and an agent over MCP
/// see the structured error, not a log line.
///
/// # Errors
///
/// `command.duplicate_method` when one of the three names is already served.
pub fn register_methods(dispatcher: &mut Dispatcher, host: Arc<Mutex<DevHost>>) -> SubResult<()> {
    let installing = Arc::clone(&host);
    dispatcher.register::<InstallParams, DevInstall, _>(
        PLUGIN_INSTALL,
        "Install a plugin from a built component or a plugin directory, optionally as a watched \
         dev install.",
        move |_, params| {
            let params: InstallParams = typed(params)?;
            let install =
                locked(&installing)?.install(params.location, &params.path, params.dev)?;
            to_value(&install)
        },
    )?;

    let reloading = Arc::clone(&host);
    dispatcher.register::<ReloadParams, ReloadStatus, _>(
        PLUGIN_RELOAD,
        "Load an installed plugin again, re-registering its commands, effects and tools.",
        move |_, params| {
            let params: ReloadParams = typed(params)?;
            to_value(&locked(&reloading)?.reload(&params.id)?)
        },
    )?;

    dispatcher.register::<StatusParams, Statuses, _>(
        PLUGIN_STATUS,
        "Report what each plugin's last load did, including the error of one that failed.",
        move |_, params| {
            typed::<StatusParams>(params)?;
            to_value(&Statuses {
                plugins: locked(&host)?.statuses(),
            })
        },
    )
}

/// The host, or the internal error a poisoned lock is.
fn locked(host: &Arc<Mutex<DevHost>>) -> SubResult<std::sync::MutexGuard<'_, DevHost>> {
    host.lock().map_err(|_| {
        SubError::new(
            sub_core::codes::INTERNAL,
            "the plugin host was left locked by a panicking call",
        )
    })
}

/// Decodes a method's parameters, reporting a mismatch the way the dispatcher
/// does.
fn typed<T: serde::de::DeserializeOwned>(params: Value) -> SubResult<T> {
    serde_json::from_value(params).map_err(|err| {
        SubError::wrap(
            sub_command::codes::INVALID_PARAMS,
            "parameters do not match the method",
            &err,
        )
    })
}

/// Serialises a method's result.
fn to_value<T: Serialize>(value: &T) -> SubResult<Value> {
    serde_json::to_value(value).map_err(|err| {
        SubError::wrap(
            sub_core::codes::INTERNAL,
            "a plugin host result could not be serialised",
            &err,
        )
    })
}

/// The JSON Schema entries of the dev methods, folded into the plugin
/// management document.
pub(crate) mod schema {
    use schemars::SchemaGenerator;
    use serde_json::Value;

    use super::{
        DevInstall, InstallParams, PLUGIN_INSTALL, PLUGIN_RELOAD, PLUGIN_STATUS, ReloadParams,
        ReloadStatus, StatusParams, Statuses,
    };

    /// One method's entry, built the way `registry::schema` builds one.
    fn method<P: schemars::JsonSchema, R: schemars::JsonSchema>(
        generator: &mut SchemaGenerator,
        name: &str,
        description: &str,
    ) -> Value {
        serde_json::json!({
            "name": name,
            "kind": "query",
            "description": description,
            "params": generator.subschema_for::<P>().to_value(),
            "result": generator.subschema_for::<R>().to_value(),
        })
    }

    /// The three dev methods, in name order.
    pub(crate) fn methods(generator: &mut SchemaGenerator) -> Vec<Value> {
        vec![
            method::<InstallParams, DevInstall>(
                generator,
                PLUGIN_INSTALL,
                "Install a plugin from a built component or a plugin directory, optionally as a \
                 watched dev install.",
            ),
            method::<ReloadParams, ReloadStatus>(
                generator,
                PLUGIN_RELOAD,
                "Load an installed plugin again, re-registering its commands, effects and tools.",
            ),
            method::<StatusParams, Statuses>(
                generator,
                PLUGIN_STATUS,
                "Report what each plugin's last load did, including the error of one that failed.",
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use sub_core::SubError;

    use super::{
        DevHost, DevInstall, DevWatcher, LinkMode, PluginArtifacts, WASM_FILE_NAME, install,
    };
    use crate::codes;
    use crate::manifest::PluginId;
    use crate::menu::CommandDesc;
    use crate::registry::{PluginDirs, PluginRegistry};

    /// The bytes of an empty but well-formed wasm header. Nothing compiles it
    /// here: the loader in these tests is the test's own.
    const WASM: &[u8] = b"\0asm\x01\0\0\0";

    /// A scratch tree: a source plugin directory and an empty user plugin
    /// directory.
    fn scratch(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("subordinate-dev-{name}"));
        std::fs::remove_dir_all(&root).ok();
        let source = root.join("source");
        std::fs::create_dir_all(source.join("schemas")).expect("a source tree");
        std::fs::write(
            source.join("plugin.toml"),
            "[plugin]\nid = \"com.example.one\"\nname = \"One\"\nversion = \"0.1.0\"\n\
             api = \"0.1\"\nworlds = [\"command\"]\n",
        )
        .expect("a manifest");
        std::fs::write(source.join("schemas").join("tool.json"), "{}").expect("a schema");
        std::fs::write(source.join("plugin.wasm"), WASM).expect("a component");
        (source, root.join("user"))
    }

    /// The id every fixture uses.
    fn id() -> PluginId {
        PluginId::parse("com.example.one").expect("a valid id")
    }

    /// A registry over a user directory.
    fn registry(user: &Path) -> Arc<PluginRegistry> {
        Arc::new(PluginRegistry::new(PluginDirs::new(user)))
    }

    /// A description as a plugin would declare it.
    fn desc(id: &str) -> CommandDesc {
        CommandDesc {
            id: id.to_owned(),
            title: id.to_owned(),
            shortcut: None,
        }
    }

    #[test]
    fn a_dev_install_links_the_whole_source_tree_and_records_where_it_came_from() {
        let (source, user) = scratch("install");
        let installed = install(&PluginDirs::new(&user), None, &source, true).expect("installed");

        assert_eq!(installed.id.as_str(), "com.example.one");
        assert!(installed.dev);
        assert!(installed.directory.join("plugin.toml").is_file());
        assert!(installed.directory.join(WASM_FILE_NAME).is_file());
        assert!(
            installed
                .directory
                .join("schemas")
                .join("tool.json")
                .is_file(),
            "a manifest's schema files come along"
        );
        assert!(installed.directory.join(super::DEV_FILE_NAME).is_file());

        // The registry sees it, and sees it as a dev install.
        let scan = registry(&user).scan().expect("a scan");
        assert_eq!(scan.plugins().len(), 1);
        assert!(scan.plugins()[0].dev, "the listing marks a dev install");
        assert_eq!(
            DevInstall::of(&scan.plugins()[0]).map(|found| found.source.root),
            Some(std::fs::canonicalize(&source).expect("a real path")),
        );
    }

    #[test]
    fn a_plain_install_is_a_snapshot_with_no_dev_record() {
        let (source, user) = scratch("plain");
        let installed = install(&PluginDirs::new(&user), None, &source, false).expect("installed");
        assert!(!installed.dev);
        assert_eq!(installed.mode, LinkMode::Copy);
        assert!(!installed.directory.join(super::DEV_FILE_NAME).exists());
        assert!(!registry(&user).scan().expect("a scan").plugins()[0].dev);
    }

    #[test]
    fn a_built_component_finds_the_manifest_above_it() {
        let (source, user) = scratch("built");
        let built = source.join("target").join("wasm32-wasip2").join("release");
        std::fs::create_dir_all(&built).expect("a build directory");
        let wasm = built.join("my_plugin.wasm");
        std::fs::write(&wasm, WASM).expect("a component");

        let installed = install(&PluginDirs::new(&user), None, &wasm, true).expect("installed");
        assert_eq!(
            installed.source.wasm,
            std::fs::canonicalize(&wasm).expect("a real path")
        );
        assert!(installed.directory.join(WASM_FILE_NAME).is_file());
    }

    #[test]
    fn a_source_that_is_not_a_plugin_is_refused() {
        let (source, user) = scratch("bad-source");
        let stray = source.parent().expect("a parent").join("stray.wasm");
        std::fs::write(&stray, WASM).expect("a stray component");
        std::fs::remove_file(source.join("plugin.toml")).expect("no manifest");

        let error =
            install(&PluginDirs::new(&user), None, &stray, true).expect_err("no manifest anywhere");
        assert_eq!(error.code, codes::DEV_SOURCE_INVALID);
    }

    #[test]
    fn installing_over_another_plugin_is_refused() {
        let (source, user) = scratch("conflict");
        let occupied = user.join("com.example.one");
        std::fs::create_dir_all(&occupied).expect("an occupant");
        std::fs::write(
            occupied.join("plugin.toml"),
            "[plugin]\nid = \"com.example.other\"\nname = \"Other\"\nversion = \"0.1.0\"\n\
             api = \"0.1\"\nworlds = [\"command\"]\n",
        )
        .expect("a manifest");

        let error =
            install(&PluginDirs::new(&user), None, &source, true).expect_err("someone is there");
        assert_eq!(error.code, codes::INSTALL_CONFLICT);
    }

    #[test]
    fn the_watcher_reports_a_rebuilt_component_once_it_settles() {
        let (source, user) = scratch("watch");
        let installed = install(&PluginDirs::new(&user), None, &source, true).expect("installed");
        let mut watcher = DevWatcher::new();
        watcher.watch(installed.id.clone(), installed.watched());
        assert!(watcher.poll().is_empty(), "nothing has changed yet");

        // A rebuild: the file grows.
        std::fs::write(&installed.source.wasm, b"\0asm\x01\0\0\0\0\0").expect("a rebuild");
        assert!(
            watcher.poll().is_empty(),
            "a file seen changing once is not reported yet"
        );
        assert_eq!(watcher.poll(), vec![installed.id.clone()]);
        assert!(watcher.poll().is_empty(), "and it is reported only once");
    }

    #[test]
    fn a_spawned_watcher_sees_a_rebuild_well_inside_a_second() {
        let (source, user) = scratch("spawn");
        let installed = install(&PluginDirs::new(&user), None, &source, true).expect("installed");
        let mut watcher = DevWatcher::new();
        watcher.watch(installed.id.clone(), installed.watched());

        let (sender, receiver) = std::sync::mpsc::channel();
        let handle = watcher.spawn(Duration::from_millis(20), move |id| {
            sender.send(id.clone()).ok();
        });

        std::fs::write(&installed.source.wasm, b"\0asm\x01\0\0\0\0\0").expect("a rebuild");
        let seen = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("the rebuild is seen within a second");
        assert_eq!(seen, installed.id);
        drop(handle);
    }

    #[test]
    fn a_reload_re_registers_commands_and_counts_a_generation() {
        let (source, user) = scratch("reload");
        let installed = install(&PluginDirs::new(&user), None, &source, true).expect("installed");
        // The loader answers whatever the component's bytes say, so a rebuild
        // changes what is registered.
        let loader = |_: &_, path: &Path| {
            let bytes = std::fs::read(path).expect("the component");
            Ok(PluginArtifacts {
                commands: (0..=(bytes.len() - WASM.len()))
                    .map(|index| desc(&format!("do_{index}")))
                    .collect(),
                ..PluginArtifacts::default()
            })
        };
        let mut host = DevHost::new(registry(&user), loader);

        let status = host.reload(&installed.id).expect("the first load");
        assert!(status.ok);
        assert_eq!(status.generation, 1);
        assert_eq!(status.commands, 1);
        assert!(status.dev);
        assert_eq!(host.commands().len(), 1);
        assert!(host.commands().get("com.example.one/do_0").is_some());

        std::fs::write(&installed.source.wasm, b"\0asm\x01\0\0\0\0").expect("a rebuild");
        let status = host.reload(&installed.id).expect("the reload");
        assert_eq!(status.generation, 2);
        assert_eq!(status.commands, 2);
        assert_eq!(
            host.commands().len(),
            2,
            "the previous registration was replaced, not added to"
        );
        assert_eq!(host.commands().plugins(), vec!["com.example.one"]);
    }

    #[test]
    fn a_failed_reload_keeps_the_previous_version_and_reports_the_error() {
        let (source, user) = scratch("failed-reload");
        let installed = install(&PluginDirs::new(&user), None, &source, true).expect("installed");
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&fail);
        let mut host = DevHost::new(registry(&user), move |_: &_, _: &Path| {
            if flag.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(SubError::new(codes::LOAD_FAILED, "not a component")
                    .with_detail("id", "com.example.one"));
            }
            Ok(PluginArtifacts {
                commands: vec![desc("do_it")],
                ..PluginArtifacts::default()
            })
        });
        host.reload(&installed.id).expect("the first load");

        fail.store(true, std::sync::atomic::Ordering::Relaxed);
        let error = host.reload(&installed.id).expect_err("the reload fails");
        assert_eq!(error.code, codes::LOAD_FAILED);

        // The previous version is still registered and still answering.
        assert_eq!(host.commands().len(), 1);
        assert!(host.commands().get("com.example.one/do_it").is_some());

        let status = host.status(&installed.id).expect("a recorded status");
        assert!(!status.ok);
        assert_eq!(status.generation, 1, "the loaded generation is unchanged");
        assert_eq!(status.commands, 1);
        let reported = status.error.as_ref().expect("the failure is recorded");
        assert_eq!(reported.code, "plugin.load_failed");
        assert_eq!(reported.details["id"], "com.example.one");
        assert_eq!(host.statuses().len(), 1);
    }

    #[test]
    fn a_missing_component_is_a_structured_failure() {
        let (source, user) = scratch("no-wasm");
        let installed = install(&PluginDirs::new(&user), None, &source, true).expect("installed");
        std::fs::remove_file(installed.directory.join(WASM_FILE_NAME)).expect("the link goes");
        let mut host = DevHost::new(registry(&user), |_: &_, _: &Path| {
            Ok(PluginArtifacts::default())
        });
        let error = host.reload(&installed.id).expect_err("nothing to load");
        assert_eq!(error.code, codes::LOAD_FAILED);
        assert!(host.status(&installed.id).is_some_and(|status| !status.ok));
    }

    #[test]
    fn reloading_something_that_is_not_installed_says_so() {
        let (_, user) = scratch("absent");
        let mut host = DevHost::new(registry(&user), |_: &_, _: &Path| {
            Ok(PluginArtifacts::default())
        });
        let error = host.reload(&id()).expect_err("nothing is installed");
        assert_eq!(error.code, codes::NOT_INSTALLED);
    }
}
