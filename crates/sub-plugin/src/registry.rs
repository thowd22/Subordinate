//! The plugin registry: where plugins are installed, which are switched on,
//! and what is listed to a user or an agent.
//!
//! A plugin is a directory holding a `plugin.toml` (docs/PLAN.md §6.3). Those
//! directories live in one of two places, and the pair is a [`PluginDirs`]:
//!
//! - the **user** directory, one per machine account, where
//!   `subordinate-cli plugin install` puts everything by default —
//!   `$XDG_DATA_HOME/subordinate/plugins` on Linux,
//!   `~/Library/Application Support/Subordinate/plugins` on macOS,
//!   `%APPDATA%\Subordinate\plugins` on Windows;
//! - the **project-local** directory, `.subordinate/plugins` beside the
//!   project file, which travels with the edit so a project can pin the
//!   plugins it needs.
//!
//! Project-local wins. When the same [`PluginId`] is installed in both, the
//! project's copy is the one that loads and the user's copy is recorded as
//! [`Shadowed`] and logged as a warning, rather than silently disappearing.
//!
//! [`PluginRegistry::scan`] walks both directories and parses every manifest it
//! finds. **A bad plugin never stops the rest**: a `plugin.toml` that is
//! missing, unreadable or invalid becomes one [`LoadFailure`] row in the scan
//! and the other plugins load as usual, which is what lets the editor start
//! with a broken plugin on disk and still say so.
//!
//! Whether a plugin is enabled is the user's choice, not the plugin's, so it
//! is kept outside the plugin directory: a small `plugins.json` in the user
//! directory lists the ids that are switched off, and everything not listed is
//! on. The registry holds no cached state — every call re-reads the disk — so
//! the GUI, the CLI and the MCP bridge see the same answer without a lock
//! between them.
//!
//! ```
//! use sub_plugin::registry::{InstallLocation, PluginDirs, PluginRegistry};
//!
//! let temp = std::env::temp_dir().join("subordinate-registry-doctest");
//! let user = temp.join("user");
//! std::fs::create_dir_all(user.join("com.example.gain")).unwrap();
//! std::fs::write(
//!     user.join("com.example.gain").join("plugin.toml"),
//!     "[plugin]\nid = \"com.example.gain\"\nname = \"Gain\"\nversion = \"0.1.0\"\n\
//!      api = \"0.1\"\nworlds = [\"audio-effect\"]\n",
//! )
//! .unwrap();
//!
//! let registry = PluginRegistry::new(PluginDirs::new(&user));
//! let scan = registry.scan().unwrap();
//! assert_eq!(scan.plugins().len(), 1);
//! assert_eq!(scan.plugins()[0].location, InstallLocation::User);
//! assert!(scan.plugins()[0].enabled);
//!
//! let id = scan.plugins()[0].id.clone();
//! registry.set_enabled(&id, false).unwrap();
//! assert!(!registry.scan().unwrap().plugins()[0].enabled);
//!
//! registry.remove(&id).unwrap();
//! assert!(registry.scan().unwrap().plugins().is_empty());
//! std::fs::remove_dir_all(&temp).ok();
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sub_command::Dispatcher;
use sub_core::{SubError, SubResult};

use crate::codes;
use crate::manifest::{MANIFEST_FILE_NAME, Manifest, PluginId};
use crate::mcp::{PublishedTool, ToolCatalog};

/// The file recording which plugins the user switched off.
pub const REGISTRY_STATE_FILE: &str = "plugins.json";

/// The version stamped into that file.
pub const REGISTRY_STATE_SCHEMA_VERSION: u32 = 1;

/// The directory a project's own plugins live in, relative to the directory
/// holding the project file.
pub const PROJECT_PLUGINS_DIR: [&str; 2] = [".subordinate", "plugins"];

/// Where a plugin is installed.
///
/// The order is the precedence order: [`InstallLocation::Project`] is greater
/// than [`InstallLocation::User`] and wins an id conflict.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum InstallLocation {
    /// The per-user plugin directory, shared by every project.
    User,
    /// `.subordinate/plugins` beside the open project file.
    Project,
}

impl InstallLocation {
    /// Both locations, in precedence order, weakest first.
    pub const ALL: [Self; 2] = [Self::User, Self::Project];

    /// The name used in JSON and on the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

impl fmt::Display for InstallLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The two directories plugins are installed in.
///
/// The user directory is always present; the project-local one exists only
/// while a project is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDirs {
    /// The per-user directory.
    user: PathBuf,
    /// `.subordinate/plugins` for the open project, when there is one.
    project: Option<PathBuf>,
}

impl PluginDirs {
    /// The directories for a user directory and no open project.
    #[must_use]
    pub fn new(user: impl Into<PathBuf>) -> Self {
        Self {
            user: user.into(),
            project: None,
        }
    }

    /// The same directories with a project-local directory added.
    #[must_use]
    pub fn with_project(mut self, project: impl Into<PathBuf>) -> Self {
        self.project = Some(project.into());
        self
    }

    /// The same directories with the project-local directory that belongs to
    /// `project_file`: `.subordinate/plugins` beside the file.
    ///
    /// A path with no parent directory — which a project file never has once
    /// it has been saved — leaves the project directory unset.
    #[must_use]
    pub fn with_project_file(self, project_file: &Path) -> Self {
        match project_file.parent() {
            Some(parent) => {
                let mut dir = parent.to_path_buf();
                dir.extend(PROJECT_PLUGINS_DIR);
                self.with_project(dir)
            }
            None => self,
        }
    }

    /// The default per-user directory and no open project.
    ///
    /// # Errors
    ///
    /// [`codes::PLUGIN_DIR_UNAVAILABLE`] when the platform offers no per-user
    /// data directory, which means neither `HOME` nor `APPDATA` is set.
    pub fn for_user() -> SubResult<Self> {
        Ok(Self::new(default_user_dir()?))
    }

    /// The per-user directory.
    #[must_use]
    pub fn user(&self) -> &Path {
        &self.user
    }

    /// The project-local directory, when a project is open.
    #[must_use]
    pub fn project(&self) -> Option<&Path> {
        self.project.as_deref()
    }

    /// The directory one location lives in, or `None` for a project-local
    /// location with no open project.
    #[must_use]
    pub fn dir(&self, location: InstallLocation) -> Option<&Path> {
        match location {
            InstallLocation::User => Some(&self.user),
            InstallLocation::Project => self.project(),
        }
    }

    /// Where the enable/disable state is kept: `plugins.json` in the user
    /// directory, because the choice is the user's and not the project's.
    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        self.user.join(REGISTRY_STATE_FILE)
    }
}

/// One installed plugin, as listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstalledPlugin {
    /// The identity from the manifest, which is the key everything uses.
    pub id: PluginId,
    /// Which of the two directories it was found in.
    pub location: InstallLocation,
    /// The directory holding its `plugin.toml`.
    #[schemars(with = "String")]
    pub directory: PathBuf,
    /// Whether it is switched on. Everything is on until switched off.
    pub enabled: bool,
    /// Whether it is a dev install: linked to a source tree, watched, and
    /// reloaded when that tree is rebuilt (see [`crate::dev`]).
    #[serde(default)]
    pub dev: bool,
    /// Its whole parsed manifest, so a listing can show what it asks for
    /// without reading the file again.
    pub manifest: Manifest,
}

impl InstalledPlugin {
    /// The display name from the manifest.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.manifest.plugin.name
    }

    /// The plugin's own version.
    #[must_use]
    pub fn version(&self) -> String {
        self.manifest.plugin.version.to_string()
    }
}

/// A plugin directory that could not be loaded.
///
/// Reported per plugin: the scan that produced it loaded every other plugin.
/// The three error fields are a [`SubError`] flattened, so the row carries a
/// stable code into JSON without the schema depending on the error type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoadFailure {
    /// The directory that failed.
    #[schemars(with = "String")]
    pub directory: PathBuf,
    /// Which of the two directories it was found in.
    pub location: InstallLocation,
    /// The stable error code, e.g. `plugin.invalid_manifest`.
    pub code: String,
    /// The one-line message.
    pub message: String,
    /// The error's details, e.g. the offending manifest field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(with = "BTreeMap<String, Value>")]
    pub details: BTreeMap<String, Value>,
}

impl LoadFailure {
    /// Records `error` against the directory it came from.
    #[must_use]
    pub fn new(directory: PathBuf, location: InstallLocation, error: &SubError) -> Self {
        Self {
            directory,
            location,
            code: error.code.as_str().to_owned(),
            message: error.message.clone(),
            details: error.details.clone(),
        }
    }
}

/// A plugin that is installed but not loaded, because another copy of the same
/// id won.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Shadowed {
    /// The id both copies claim.
    pub id: PluginId,
    /// The directory of the copy that is *not* used.
    #[schemars(with = "String")]
    pub directory: PathBuf,
    /// Where that ignored copy lives.
    pub location: InstallLocation,
    /// The directory of the copy that won.
    #[schemars(with = "String")]
    pub shadowed_by: PathBuf,
    /// Where the winning copy lives.
    pub shadowed_by_location: InstallLocation,
}

/// Everything one walk of the plugin directories found.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Scan {
    /// The plugins that loaded, sorted by id.
    plugins: Vec<InstalledPlugin>,
    /// The directories that did not, sorted by path.
    failures: Vec<LoadFailure>,
    /// The copies that lost an id conflict, sorted by path.
    shadowed: Vec<Shadowed>,
}

impl Scan {
    /// The plugins that loaded, sorted by id.
    #[must_use]
    pub fn plugins(&self) -> &[InstalledPlugin] {
        &self.plugins
    }

    /// The plugin directories that failed to load.
    #[must_use]
    pub fn failures(&self) -> &[LoadFailure] {
        &self.failures
    }

    /// The copies hidden by an id conflict.
    #[must_use]
    pub fn shadowed(&self) -> &[Shadowed] {
        &self.shadowed
    }

    /// The plugin with this id, if it is installed.
    #[must_use]
    pub fn get(&self, id: &PluginId) -> Option<&InstalledPlugin> {
        self.plugins.iter().find(|plugin| &plugin.id == id)
    }

    /// The plugins that are switched on, in id order.
    pub fn enabled(&self) -> impl Iterator<Item = &InstalledPlugin> {
        self.plugins.iter().filter(|plugin| plugin.enabled)
    }
}

/// What `plugin.enable` or `plugin.disable` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnableChange {
    /// The plugin acted on.
    pub id: PluginId,
    /// Whether it is switched on now.
    pub enabled: bool,
    /// False when it was already in that state, so a caller can tell a change
    /// from a no-op.
    pub changed: bool,
}

/// What `plugin.remove` deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Removal {
    /// The plugin removed.
    pub id: PluginId,
    /// Where it was installed.
    pub location: InstallLocation,
    /// The directory that was deleted.
    #[schemars(with = "String")]
    pub directory: PathBuf,
}

/// The result of `plugin.tools`: every MCP tool the enabled plugins contribute.
///
/// The MCP bridge asks for this when a client lists tools, and publishes each
/// row beside its own tools under the plugin-id prefix the row carries
/// (docs/PLAN.md §6.2, §7). Nothing here is loaded from a component: the
/// manifest is what the user approved on install, so a tool is listed as the
/// manifest declares it whether or not the plugin is instantiated yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolListing {
    /// The tools, sorted by published name.
    pub tools: Vec<PublishedTool>,
    /// The plugins whose tools could not be published, sorted by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<ToolFailure>,
}

/// One plugin whose declared tools could not be published.
///
/// Reported per plugin, like a [`LoadFailure`]: a plugin with an unreadable or
/// uncompilable tool schema hides its own tools and nobody else's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolFailure {
    /// The plugin whose tools are missing from the listing.
    pub id: PluginId,
    /// The stable error code, e.g. `plugin.tool_schema_unreadable`.
    pub code: String,
    /// The one-line message.
    pub message: String,
    /// The error's details, e.g. the offending tool.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(with = "BTreeMap<String, Value>")]
    pub details: BTreeMap<String, Value>,
}

impl ToolFailure {
    /// Records `error` against the plugin it came from.
    #[must_use]
    pub fn new(id: PluginId, error: &SubError) -> Self {
        Self {
            id,
            code: error.code.as_str().to_owned(),
            message: error.message.clone(),
            details: error.details.clone(),
        }
    }
}

/// The tools every enabled plugin in `scan` contributes.
///
/// Two plugins cannot normally collide on a published name — the prefix is the
/// plugin id — but ids that differ only in where a dot falls would, so a
/// colliding row is dropped and reported rather than silently shadowing the
/// one already published.
#[must_use]
pub fn contributed_tools(scan: &Scan) -> ToolListing {
    let mut listing = ToolListing::default();
    let mut names: BTreeMap<String, PluginId> = BTreeMap::new();
    for plugin in scan.enabled() {
        if plugin.manifest.mcp.tools.is_empty() {
            continue;
        }
        let catalog = match ToolCatalog::from_manifest(&plugin.manifest, &plugin.directory) {
            Ok(catalog) => catalog,
            Err(error) => {
                listing
                    .failures
                    .push(ToolFailure::new(plugin.id.clone(), &error));
                continue;
            }
        };
        for tool in catalog.published() {
            if let Some(owner) = names.get(&tool.name) {
                listing.failures.push(ToolFailure::new(
                    plugin.id.clone(),
                    &SubError::new(
                        codes::DUPLICATE_TOOL,
                        "two plugins publish the same MCP tool name",
                    )
                    .with_detail("tool", tool.name.clone())
                    .with_detail("published_by", owner.as_str().to_owned()),
                ));
                continue;
            }
            names.insert(tool.name.clone(), plugin.id.clone());
            listing.tools.push(tool);
        }
    }
    listing
        .tools
        .sort_by(|left, right| left.name.cmp(&right.name));
    listing
        .failures
        .sort_by(|left, right| left.id.cmp(&right.id));
    listing
}

/// The enable/disable state, as it is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RegistryState {
    /// The version of this file's shape.
    schema_version: u32,
    /// The ids that are switched off. Everything else is on.
    #[serde(default)]
    disabled: BTreeSet<PluginId>,
}

impl Default for RegistryState {
    fn default() -> Self {
        Self {
            schema_version: REGISTRY_STATE_SCHEMA_VERSION,
            disabled: BTreeSet::new(),
        }
    }
}

impl RegistryState {
    /// Reads the state at `path`, treating a missing file as "nothing is
    /// switched off".
    fn load(path: &Path) -> SubResult<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => {
                return Err(SubError::wrap(
                    codes::REGISTRY_STATE_UNREADABLE,
                    "could not read the plugin registry state",
                    &err,
                )
                .with_detail("path", path.display().to_string()));
            }
        };
        let state: Self = serde_json::from_str(&text).map_err(|err| {
            SubError::wrap(
                codes::INVALID_REGISTRY_STATE,
                "the plugin registry state is not the JSON this build reads",
                &err,
            )
            .with_detail("path", path.display().to_string())
        })?;
        if state.schema_version != REGISTRY_STATE_SCHEMA_VERSION {
            return Err(SubError::new(
                codes::INVALID_REGISTRY_STATE,
                "the plugin registry state was written by a different version",
            )
            .with_detail("path", path.display().to_string())
            .with_detail("schema_version", state.schema_version)
            .with_detail("expected", REGISTRY_STATE_SCHEMA_VERSION));
        }
        Ok(state)
    }

    /// Writes the state to `path`, creating the directory it lives in.
    fn save(&self, path: &Path) -> SubResult<()> {
        let unwritable = |err: &std::io::Error| {
            SubError::wrap(
                codes::REGISTRY_STATE_UNWRITABLE,
                "could not record the plugin registry state",
                err,
            )
            .with_detail("path", path.display().to_string())
        };
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|err| unwritable(&err))?;
        }
        let mut text = serde_json::to_string_pretty(self).map_err(|err| {
            SubError::wrap(
                codes::INVALID_REGISTRY_STATE,
                "the plugin registry state could not be serialised",
                &err,
            )
        })?;
        text.push('\n');
        std::fs::write(path, text).map_err(|err| unwritable(&err))
    }
}

/// The registry over one pair of plugin directories.
///
/// It caches nothing: every method re-reads the directories and the state
/// file, so two processes never disagree and no lock is needed between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRegistry {
    dirs: PluginDirs,
}

impl PluginRegistry {
    /// The registry over `dirs`.
    #[must_use]
    pub const fn new(dirs: PluginDirs) -> Self {
        Self { dirs }
    }

    /// The directories it walks.
    #[must_use]
    pub const fn dirs(&self) -> &PluginDirs {
        &self.dirs
    }

    /// Walks both directories, parsing every manifest.
    ///
    /// A plugin directory that fails to load becomes a [`LoadFailure`] and the
    /// walk goes on, so one bad plugin never costs the others (docs/PLAN.md
    /// §6.4). A directory holding no `plugin.toml` at all is not a plugin and
    /// is passed over in silence.
    ///
    /// # Errors
    ///
    /// [`codes::PLUGIN_DIR_UNREADABLE`] when a plugin directory exists but
    /// cannot be listed, and the [`codes::REGISTRY_STATE_UNREADABLE`] or
    /// [`codes::INVALID_REGISTRY_STATE`] of a `plugins.json` this build cannot
    /// read. A directory that is simply not there is empty, not an error.
    pub fn scan(&self) -> SubResult<Scan> {
        let disabled = RegistryState::load(&self.dirs.state_path())?.disabled;

        let mut plugins: BTreeMap<PluginId, InstalledPlugin> = BTreeMap::new();
        let mut failures = Vec::new();
        let mut shadowed = Vec::new();

        for location in InstallLocation::ALL {
            let Some(dir) = self.dirs.dir(location) else {
                continue;
            };
            for directory in candidates(dir)? {
                let manifest = match Manifest::read_dir(&directory) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        tracing::warn!(
                            directory = %directory.display(),
                            code = error.code.as_str(),
                            "a plugin could not be loaded: {}",
                            error.message,
                        );
                        failures.push(LoadFailure::new(directory, location, &error));
                        continue;
                    }
                };
                let id = manifest.plugin.id.clone();
                let enabled = !disabled.contains(&id);
                let dev = directory.join(crate::dev::DEV_FILE_NAME).is_file();
                let plugin = InstalledPlugin {
                    id: id.clone(),
                    location,
                    directory,
                    enabled,
                    dev,
                    manifest,
                };
                if let Some(previous) = plugins.insert(id, plugin) {
                    // `ALL` is weakest first and one directory is walked in
                    // path order, so whatever was already there is the loser.
                    let winner = &plugins[&previous.id];
                    tracing::warn!(
                        id = previous.id.as_str(),
                        ignored = %previous.directory.display(),
                        used = %winner.directory.display(),
                        "two plugins claim one id; the {} copy wins",
                        winner.location,
                    );
                    shadowed.push(Shadowed {
                        id: previous.id,
                        directory: previous.directory,
                        location: previous.location,
                        shadowed_by: winner.directory.clone(),
                        shadowed_by_location: winner.location,
                    });
                }
            }
        }

        failures.sort_by(|left, right| left.directory.cmp(&right.directory));
        shadowed.sort_by(|left, right| left.directory.cmp(&right.directory));
        Ok(Scan {
            plugins: plugins.into_values().collect(),
            failures,
            shadowed,
        })
    }

    /// Switches a plugin on or off.
    ///
    /// The choice is recorded in the user directory's `plugins.json`, so it
    /// outlives the process and applies wherever the plugin is installed.
    ///
    /// # Errors
    ///
    /// [`codes::NOT_INSTALLED`] when no loaded plugin has that id, plus
    /// whatever [`PluginRegistry::scan`] and writing the state file return.
    pub fn set_enabled(&self, id: &PluginId, enabled: bool) -> SubResult<EnableChange> {
        let scan = self.scan()?;
        if scan.get(id).is_none() {
            return Err(not_installed(id));
        }
        let path = self.dirs.state_path();
        let mut state = RegistryState::load(&path)?;
        let changed = if enabled {
            state.disabled.remove(id)
        } else {
            state.disabled.insert(id.clone())
        };
        if changed {
            state.save(&path)?;
        }
        Ok(EnableChange {
            id: id.clone(),
            enabled,
            changed,
        })
    }

    /// Deletes an installed plugin's directory and forgets its enable state.
    ///
    /// Only the directory the scan found is deleted, so a `plugin.toml`
    /// outside the two plugin directories is never reachable from here.
    ///
    /// # Errors
    ///
    /// [`codes::NOT_INSTALLED`] when no loaded plugin has that id, and
    /// [`codes::REMOVE_FAILED`] when the directory cannot be deleted.
    pub fn remove(&self, id: &PluginId) -> SubResult<Removal> {
        let scan = self.scan()?;
        let plugin = scan.get(id).ok_or_else(|| not_installed(id))?;
        std::fs::remove_dir_all(&plugin.directory).map_err(|err| {
            SubError::wrap(
                codes::REMOVE_FAILED,
                "the plugin directory could not be deleted",
                &err,
            )
            .with_detail("id", id.as_str())
            .with_detail("path", plugin.directory.display().to_string())
        })?;

        let path = self.dirs.state_path();
        let mut state = RegistryState::load(&path)?;
        if state.disabled.remove(id) {
            state.save(&path)?;
        }
        Ok(Removal {
            id: id.clone(),
            location: plugin.location,
            directory: plugin.directory.clone(),
        })
    }
}

/// The error a method answers for an id nothing is installed under.
fn not_installed(id: &PluginId) -> SubError {
    SubError::new(codes::NOT_INSTALLED, "no plugin is installed under that id")
        .with_detail("id", id.as_str())
}

/// Every immediate subdirectory of `dir` that holds a `plugin.toml`, in path
/// order.
///
/// A missing directory yields nothing: not having installed any plugin yet is
/// the ordinary case, not a failure.
fn candidates(dir: &Path) -> SubResult<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(SubError::wrap(
                codes::PLUGIN_DIR_UNREADABLE,
                "the plugin directory could not be listed",
                &err,
            )
            .with_detail("path", dir.display().to_string()));
        }
    };

    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            SubError::wrap(
                codes::PLUGIN_DIR_UNREADABLE,
                "a plugin directory entry could not be read",
                &err,
            )
            .with_detail("path", dir.display().to_string())
        })?;
        let path = entry.path();
        if path.is_dir() && path.join(MANIFEST_FILE_NAME).exists() {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// The per-user plugin directory this platform uses.
///
/// It is the platform's data directory, not its config or runtime directory:
/// an installed plugin is data the user keeps, and it can be large.
///
/// # Errors
///
/// [`codes::PLUGIN_DIR_UNAVAILABLE`] when the platform offers no per-user data
/// directory.
pub fn default_user_dir() -> SubResult<PathBuf> {
    Ok(data_directory()?.join("plugins"))
}

/// `%APPDATA%\Subordinate`.
#[cfg(windows)]
fn data_directory() -> SubResult<PathBuf> {
    non_empty_var("APPDATA")
        .map(|base| PathBuf::from(base).join("Subordinate"))
        .ok_or_else(|| {
            SubError::new(
                codes::PLUGIN_DIR_UNAVAILABLE,
                "APPDATA is not set, so there is no per-user plugin directory",
            )
        })
}

/// `~/Library/Application Support/Subordinate`.
#[cfg(target_os = "macos")]
fn data_directory() -> SubResult<PathBuf> {
    non_empty_var("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Subordinate")
        })
        .ok_or_else(|| {
            SubError::new(
                codes::PLUGIN_DIR_UNAVAILABLE,
                "HOME is not set, so there is no per-user plugin directory",
            )
        })
}

/// `$XDG_DATA_HOME/subordinate`, or `~/.local/share/subordinate`.
#[cfg(all(unix, not(target_os = "macos")))]
fn data_directory() -> SubResult<PathBuf> {
    if let Some(base) = non_empty_var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(base).join("subordinate"));
    }
    non_empty_var("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("subordinate")
        })
        .ok_or_else(|| {
            SubError::new(
                codes::PLUGIN_DIR_UNAVAILABLE,
                "neither XDG_DATA_HOME nor HOME is set, so there is no per-user plugin directory",
            )
        })
}

/// An environment variable, treating an empty value as unset.
fn non_empty_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

/// `plugin.list`: every installed plugin, with the failures and conflicts.
pub const PLUGIN_LIST: &str = "plugin.list";
/// `plugin.enable`: switch one plugin on.
pub const PLUGIN_ENABLE: &str = "plugin.enable";
/// `plugin.disable`: switch one plugin off.
pub const PLUGIN_DISABLE: &str = "plugin.disable";
/// `plugin.remove`: delete one plugin from disk.
pub const PLUGIN_REMOVE: &str = "plugin.remove";
/// `plugin.tools`: every MCP tool the enabled plugins contribute.
pub const PLUGIN_TOOLS: &str = "plugin.tools";

/// The parameters of `plugin.list`: none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {}

/// The parameters of `plugin.tools`: none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolsParams {}

/// The parameters of `plugin.enable`, `plugin.disable` and `plugin.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginParams {
    /// The plugin's reverse-DNS id.
    pub id: PluginId,
}

/// Puts `plugin.list`, `plugin.enable`, `plugin.disable`, `plugin.remove` and
/// `plugin.tools` on a [`Dispatcher`].
///
/// They are queries, not commands: switching a plugin off changes what the
/// host loads, never the project, so nothing goes on the undo stack (decision-7
/// governs project mutations). The GUI and `subordinate-cli serve` call this so
/// the MCP bridge and any other Command API client manage plugins through the
/// same surface a user does.
///
/// # Errors
///
/// `command.duplicate_method` when one of the five names is already served.
pub fn register_methods(
    dispatcher: &mut Dispatcher,
    registry: Arc<PluginRegistry>,
) -> SubResult<()> {
    let listing = Arc::clone(&registry);
    dispatcher.register::<ListParams, Scan, _>(
        PLUGIN_LIST,
        "List every installed plugin, with the ones that failed to load and the ones hidden by \
         an id conflict.",
        move |_, params| {
            typed::<ListParams>(params)?;
            to_value(&listing.scan()?)
        },
    )?;

    let enabling = Arc::clone(&registry);
    dispatcher.register::<PluginParams, EnableChange, _>(
        PLUGIN_ENABLE,
        "Switch an installed plugin on.",
        move |_, params| {
            let params: PluginParams = typed(params)?;
            to_value(&enabling.set_enabled(&params.id, true)?)
        },
    )?;

    let disabling = Arc::clone(&registry);
    dispatcher.register::<PluginParams, EnableChange, _>(
        PLUGIN_DISABLE,
        "Switch an installed plugin off without removing it.",
        move |_, params| {
            let params: PluginParams = typed(params)?;
            to_value(&disabling.set_enabled(&params.id, false)?)
        },
    )?;

    let removing = Arc::clone(&registry);
    dispatcher.register::<PluginParams, Removal, _>(
        PLUGIN_REMOVE,
        "Delete an installed plugin's directory from disk.",
        move |_, params| {
            let params: PluginParams = typed(params)?;
            to_value(&removing.remove(&params.id)?)
        },
    )?;

    dispatcher.register::<ToolsParams, ToolListing, _>(
        PLUGIN_TOOLS,
        "List every MCP tool the enabled plugins contribute, each under its plugin's id prefix.",
        move |_, params| {
            typed::<ToolsParams>(params)?;
            to_value(&contributed_tools(&registry.scan()?))
        },
    )
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
            "a plugin registry result could not be serialised",
            &err,
        )
    })
}

/// The exported JSON Schema of the plugin management methods.
///
/// The MCP bridge builds its `plugin_*` tools from the committed copy of this
/// document, exactly as it builds the rest from `docs/schema/command-api.json`,
/// so a tool's name, description and parameter schema come from the code that
/// serves it rather than from a hand-written list (docs/PLAN.md §7).
pub mod schema {
    use schemars::generate::SchemaSettings;
    use schemars::{JsonSchema, SchemaGenerator};
    use serde_json::{Map, Value};

    use super::{
        EnableChange, ListParams, PLUGIN_DISABLE, PLUGIN_ENABLE, PLUGIN_LIST, PLUGIN_REMOVE,
        PLUGIN_TOOLS, PluginParams, Removal, Scan, ToolListing, ToolsParams,
    };

    /// The meta-schema the exported document conforms to.
    pub const META_SCHEMA: &str = "https://json-schema.org/draft/2020-12/schema";

    /// The title of the exported document.
    pub const TITLE: &str = "Subordinate plugin management API";

    /// The path of the committed copy, relative to the repository root.
    pub const COMMITTED_PATH: &str = "docs/schema/plugin-api.json";

    /// One method's entry in the document.
    fn method<P: JsonSchema, R: JsonSchema>(
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

    /// The JSON Schema of every plugin management method.
    #[must_use]
    pub fn document() -> Value {
        let mut generator = SchemaSettings::draft2020_12().into_generator();
        let methods = vec![
            method::<PluginParams, EnableChange>(
                &mut generator,
                PLUGIN_DISABLE,
                "Switch an installed plugin off without removing it.",
            ),
            method::<PluginParams, EnableChange>(
                &mut generator,
                PLUGIN_ENABLE,
                "Switch an installed plugin on.",
            ),
            method::<ListParams, Scan>(
                &mut generator,
                PLUGIN_LIST,
                "List every installed plugin, with the ones that failed to load and the ones \
                 hidden by an id conflict.",
            ),
            method::<PluginParams, Removal>(
                &mut generator,
                PLUGIN_REMOVE,
                "Delete an installed plugin's directory from disk.",
            ),
            method::<ToolsParams, ToolListing>(
                &mut generator,
                PLUGIN_TOOLS,
                "List every MCP tool the enabled plugins contribute, each under its plugin's id \
                 prefix.",
            ),
        ];
        // The dev-install and hot-reload methods are served by the same host
        // and belong in the same document, so an MCP bridge builds
        // plugin_install, plugin_reload and plugin_status from it too.
        let mut methods = methods;
        methods.extend(crate::dev::schema::methods(&mut generator));
        methods.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
        let defs = generator.take_definitions(true);
        sorted(serde_json::json!({
            "$schema": META_SCHEMA,
            "title": TITLE,
            "description": "The plugin management methods of the Subordinate Command API \
                            (docs/PLAN.md §6.4). Generated from the Rust registry types; do not \
                            edit by hand.",
            "methods": methods,
            "$defs": defs,
        }))
    }

    /// The document as the text committed under `docs/schema/`.
    #[must_use]
    pub fn document_text() -> String {
        let mut text = serde_json::to_string_pretty(&document()).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Rebuilds `value` with every object's keys in sorted order, so the
    /// committed file is byte-stable whatever `serde_json` features the build
    /// turns on.
    fn sorted(value: Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut entries: Vec<(String, Value)> = object.into_iter().collect();
                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key, sorted(value)))
                        .collect::<Map<String, Value>>(),
                )
            }
            Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
            other => other,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{COMMITTED_PATH, META_SCHEMA, TITLE, document, document_text};

        #[test]
        fn the_document_lists_every_method_in_name_order() {
            let document = document();
            assert_eq!(document["$schema"], META_SCHEMA);
            assert_eq!(document["title"], TITLE);
            let names: Vec<&str> = document["methods"]
                .as_array()
                .unwrap()
                .iter()
                .map(|method| method["name"].as_str().unwrap())
                .collect();
            assert_eq!(
                names,
                [
                    "plugin.disable",
                    "plugin.enable",
                    "plugin.install",
                    "plugin.list",
                    "plugin.reload",
                    "plugin.remove",
                    "plugin.status",
                    "plugin.tools",
                ]
            );
        }

        #[test]
        fn every_reference_resolves_against_the_definitions() {
            let document = document();
            let defs = document["$defs"].as_object().unwrap();
            for method in document["methods"].as_array().unwrap() {
                for slot in ["params", "result"] {
                    if let Some(reference) = method[slot]["$ref"].as_str() {
                        let name = reference.strip_prefix("#/$defs/").unwrap();
                        assert!(defs.contains_key(name), "$defs has no {name}");
                    }
                }
            }
            assert!(defs.contains_key("Scan"));
            assert!(defs.contains_key("PluginId"));
        }

        /// The committed copy is what the MCP bridge compiles in, so it must
        /// match what this build generates. `SUB_UPDATE_SCHEMA=1` rewrites it.
        #[test]
        fn committed_schema_is_up_to_date() {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(COMMITTED_PATH);
            let generated = document_text();
            if std::env::var_os("SUB_UPDATE_SCHEMA").is_some() {
                std::fs::write(&path, &generated).unwrap();
            }
            let committed = std::fs::read_to_string(&path).unwrap_or_default();
            assert_eq!(
                committed, generated,
                "the committed {COMMITTED_PATH} is stale; regenerate it with \
                 SUB_UPDATE_SCHEMA=1 cargo test -p sub-plugin committed_schema_is_up_to_date"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use sub_command::Dispatcher;
    use sub_edit::Engine;
    use sub_model::Project;

    use super::{
        InstallLocation, PLUGIN_DISABLE, PLUGIN_ENABLE, PLUGIN_LIST, PLUGIN_REMOVE, PLUGIN_TOOLS,
        PluginDirs, PluginRegistry, REGISTRY_STATE_FILE, contributed_tools, default_user_dir,
        register_methods,
    };
    use crate::manifest::{PluginId, World};

    /// A unique scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("subordinate-registry-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// Writes a plugin directory holding a valid manifest.
    fn install(root: &Path, id: &str, name: &str) -> PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).expect("a plugin directory");
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "[plugin]\nid = \"{id}\"\nname = \"{name}\"\nversion = \"0.1.0\"\n\
                 api = \"0.1\"\nworlds = [\"command\"]\n"
            ),
        )
        .expect("a manifest");
        dir
    }

    /// The registry over a user directory and a project directory.
    fn registry(root: &Path) -> PluginRegistry {
        PluginRegistry::new(PluginDirs::new(root.join("user")).with_project(root.join("project")))
    }

    /// Writes a plugin directory whose manifest contributes one MCP tool.
    fn install_with_tool(root: &Path, id: &str, tool: &str, schema: &str) -> PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).expect("a plugin directory");
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "[plugin]\nid = \"{id}\"\nname = \"Tooled\"\nversion = \"0.1.0\"\n\
                 api = \"0.1\"\nworlds = [\"command\", \"mcp-tools\"]\n\n\
                 [mcp.tools.{tool}]\ndescription = \"Does {tool}\"\nschema = \"{tool}.json\"\n"
            ),
        )
        .expect("a manifest");
        std::fs::write(dir.join(format!("{tool}.json")), schema).expect("a schema");
        dir
    }

    /// A schema every tool in these tests uses.
    const TOOL_SCHEMA: &str = r#"{"type":"object","properties":{"gain":{"type":"number"}}}"#;

    #[test]
    fn a_missing_plugin_directory_lists_nothing() {
        let root = scratch("missing");
        let scan = registry(&root)
            .scan()
            .expect("an absent directory is empty");
        assert!(scan.plugins().is_empty());
        assert!(scan.failures().is_empty());
        assert!(scan.shadowed().is_empty());
    }

    #[test]
    fn plugins_load_from_both_directories() {
        let root = scratch("both");
        install(&root.join("user"), "com.example.one", "One");
        install(&root.join("project"), "com.example.two", "Two");

        let scan = registry(&root).scan().expect("a scan");
        let ids: Vec<&str> = scan
            .plugins()
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect();
        assert_eq!(ids, ["com.example.one", "com.example.two"]);
        assert_eq!(scan.plugins()[0].location, InstallLocation::User);
        assert_eq!(scan.plugins()[1].location, InstallLocation::Project);
        assert_eq!(scan.plugins()[0].name(), "One");
        assert_eq!(scan.plugins()[0].version(), "0.1.0");
        assert!(scan.plugins()[0].manifest.declares(World::Command));
        assert_eq!(scan.enabled().count(), 2);
    }

    #[test]
    fn a_project_local_plugin_wins_an_id_conflict_and_the_other_is_reported() {
        let root = scratch("conflict");
        let hidden = install(&root.join("user"), "com.example.dup", "User copy");
        let winner = install(&root.join("project"), "com.example.dup", "Project copy");

        let scan = registry(&root).scan().expect("a scan");
        assert_eq!(scan.plugins().len(), 1);
        assert_eq!(scan.plugins()[0].location, InstallLocation::Project);
        assert_eq!(scan.plugins()[0].directory, winner);
        assert_eq!(scan.plugins()[0].name(), "Project copy");

        assert_eq!(scan.shadowed().len(), 1);
        let shadowed = &scan.shadowed()[0];
        assert_eq!(shadowed.id.as_str(), "com.example.dup");
        assert_eq!(shadowed.directory, hidden);
        assert_eq!(shadowed.location, InstallLocation::User);
        assert_eq!(shadowed.shadowed_by, winner);
        assert_eq!(shadowed.shadowed_by_location, InstallLocation::Project);
    }

    #[test]
    fn a_broken_manifest_is_one_failure_and_the_rest_still_load() {
        let root = scratch("broken");
        let user = root.join("user");
        install(&user, "com.example.good", "Good");
        let bad = user.join("broken");
        std::fs::create_dir_all(&bad).expect("a directory");
        std::fs::write(bad.join("plugin.toml"), "[plugin]\nid = \"nodots\"\n").expect("a manifest");
        // A directory with no manifest at all is not a plugin, not a failure.
        std::fs::create_dir_all(user.join("not-a-plugin")).expect("a directory");

        let scan = registry(&root).scan().expect("a scan");
        assert_eq!(scan.plugins().len(), 1);
        assert_eq!(scan.plugins()[0].id.as_str(), "com.example.good");
        assert_eq!(scan.failures().len(), 1);
        assert_eq!(scan.failures()[0].directory, bad);
        assert_eq!(scan.failures()[0].location, InstallLocation::User);
        assert!(scan.failures()[0].code.starts_with("plugin."));
        assert!(!scan.failures()[0].message.is_empty());
    }

    #[test]
    fn disabling_and_enabling_round_trips_through_the_state_file() {
        let root = scratch("enable");
        install(&root.join("user"), "com.example.one", "One");
        let registry = registry(&root);
        let id = PluginId::parse("com.example.one").expect("a valid id");

        let off = registry.set_enabled(&id, false).expect("disabled");
        assert!(!off.enabled);
        assert!(off.changed);
        assert!(root.join("user").join(REGISTRY_STATE_FILE).exists());
        assert!(!registry.scan().expect("a scan").plugins()[0].enabled);
        assert_eq!(registry.scan().expect("a scan").enabled().count(), 0);

        let again = registry.set_enabled(&id, false).expect("still disabled");
        assert!(!again.changed);

        let on = registry.set_enabled(&id, true).expect("enabled");
        assert!(on.enabled);
        assert!(on.changed);
        assert!(registry.scan().expect("a scan").plugins()[0].enabled);
    }

    #[test]
    fn an_unknown_id_is_reported_with_a_stable_code() {
        let root = scratch("unknown");
        let registry = registry(&root);
        let id = PluginId::parse("com.example.absent").expect("a valid id");
        let error = registry.set_enabled(&id, false).expect_err("not installed");
        assert_eq!(error.code.as_str(), "plugin.not_installed");
        assert_eq!(error.details["id"], "com.example.absent");
        assert_eq!(
            registry.remove(&id).expect_err("not installed").code,
            error.code
        );
    }

    #[test]
    fn removing_deletes_the_directory_and_forgets_the_state() {
        let root = scratch("remove");
        let dir = install(&root.join("user"), "com.example.one", "One");
        install(&root.join("user"), "com.example.two", "Two");
        let registry = registry(&root);
        let id = PluginId::parse("com.example.one").expect("a valid id");
        registry.set_enabled(&id, false).expect("disabled");

        let removal = registry.remove(&id).expect("removed");
        assert_eq!(removal.id, id);
        assert_eq!(removal.directory, dir);
        assert_eq!(removal.location, InstallLocation::User);
        assert!(!dir.exists());

        let scan = registry.scan().expect("a scan");
        assert_eq!(scan.plugins().len(), 1);
        assert_eq!(scan.plugins()[0].id.as_str(), "com.example.two");

        // Reinstalling under the same id comes back enabled: removal forgot it.
        install(&root.join("user"), "com.example.one", "One again");
        assert!(
            registry
                .scan()
                .expect("a scan")
                .get(&id)
                .expect("back")
                .enabled
        );
    }

    #[test]
    fn a_state_file_this_build_cannot_read_is_reported() {
        let root = scratch("state");
        let user = root.join("user");
        install(&user, "com.example.one", "One");
        std::fs::write(user.join(REGISTRY_STATE_FILE), "not json").expect("a state file");
        let error = registry(&root).scan().expect_err("unreadable state");
        assert_eq!(error.code.as_str(), "plugin.invalid_registry_state");

        std::fs::write(
            user.join(REGISTRY_STATE_FILE),
            r#"{"schema_version": 99, "disabled": []}"#,
        )
        .expect("a state file");
        let error = registry(&root).scan().expect_err("a future version");
        assert_eq!(error.code.as_str(), "plugin.invalid_registry_state");
    }

    #[test]
    fn the_project_directory_is_derived_from_the_project_file() {
        let dirs =
            PluginDirs::new("/data/plugins").with_project_file(Path::new("/edits/doc/doc.subproj"));
        let project = dirs.project().expect("a project directory");
        assert!(project.ends_with(Path::new(".subordinate").join("plugins")));
        assert!(project.starts_with("/edits/doc"));
        assert_eq!(
            dirs.dir(InstallLocation::User),
            Some(Path::new("/data/plugins"))
        );
        assert_eq!(dirs.state_path(), Path::new("/data/plugins/plugins.json"));
        assert!(PluginDirs::new("/data/plugins").project().is_none());
    }

    #[test]
    fn the_default_user_directory_is_under_the_platform_data_directory() {
        let dir = default_user_dir().expect("a data directory in a test environment");
        assert!(dir.ends_with("plugins"), "{}", dir.display());
        assert!(dir.is_absolute(), "{}", dir.display());
    }

    #[test]
    fn the_command_api_serves_the_four_methods() {
        let root = scratch("api");
        install(&root.join("user"), "com.example.one", "One");
        let engine = Engine::spawn(Project::new("Doc cut")).expect("an engine");
        let mut dispatcher = Dispatcher::new(engine.handle().clone());
        register_methods(&mut dispatcher, Arc::new(registry(&root))).expect("the methods");

        for method in [PLUGIN_LIST, PLUGIN_ENABLE, PLUGIN_DISABLE, PLUGIN_REMOVE] {
            assert!(dispatcher.contains(method), "{method} is not served");
        }

        let listed = dispatcher.invoke(PLUGIN_LIST, None).expect("a listing");
        assert_eq!(listed["plugins"][0]["id"], "com.example.one");
        assert_eq!(listed["plugins"][0]["enabled"], true);
        assert_eq!(listed["plugins"][0]["location"], "user");

        let params = serde_json::json!({ "id": "com.example.one" });
        let disabled = dispatcher
            .invoke(PLUGIN_DISABLE, Some(params.clone()))
            .expect("disabled");
        assert_eq!(disabled["enabled"], false);
        assert_eq!(disabled["changed"], true);
        let listed = dispatcher.invoke(PLUGIN_LIST, None).expect("a listing");
        assert_eq!(listed["plugins"][0]["enabled"], false);

        dispatcher
            .invoke(PLUGIN_ENABLE, Some(params.clone()))
            .expect("enabled");
        let removed = dispatcher
            .invoke(PLUGIN_REMOVE, Some(params))
            .expect("removed");
        assert_eq!(removed["id"], "com.example.one");
        let listed = dispatcher.invoke(PLUGIN_LIST, None).expect("a listing");
        assert!(listed["plugins"].as_array().expect("an array").is_empty());

        let error = dispatcher
            .invoke(
                PLUGIN_ENABLE,
                Some(serde_json::json!({ "id": "com.example.one" })),
            )
            .expect_err("no longer installed");
        assert_eq!(error.code.as_str(), "plugin.not_installed");

        let error = dispatcher
            .invoke(PLUGIN_ENABLE, Some(serde_json::json!({ "nope": 1 })))
            .expect_err("bad parameters");
        assert_eq!(error.code.as_str(), "command.invalid_params");

        engine.shutdown().expect("a clean shutdown");
    }

    #[test]
    fn contributed_tools_are_published_under_each_plugin_id() {
        let root = scratch("tools");
        let user = root.join("user");
        install_with_tool(&user, "com.example.one", "cut_silence", TOOL_SCHEMA);
        install_with_tool(&user, "com.example.two", "tint", TOOL_SCHEMA);
        // A plugin with no tools contributes none, and is not a failure.
        install(&user, "com.example.plain", "Plain");

        let listing = contributed_tools(&registry(&root).scan().expect("a scan"));
        assert!(listing.failures.is_empty());
        let names: Vec<&str> = listing
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["com_example_one_cut_silence", "com_example_two_tint"]
        );
        assert_eq!(listing.tools[0].plugin, "com.example.one");
        assert_eq!(listing.tools[0].tool, "cut_silence");
        assert_eq!(listing.tools[0].title, "com.example.one.cut_silence");
        assert_eq!(listing.tools[0].description, "Does cut_silence");
    }

    #[test]
    fn a_disabled_plugin_contributes_nothing() {
        let root = scratch("tools-disabled");
        install_with_tool(
            &root.join("user"),
            "com.example.one",
            "cut_silence",
            TOOL_SCHEMA,
        );
        let registry = registry(&root);
        let id = PluginId::parse("com.example.one").expect("a valid id");
        registry.set_enabled(&id, false).expect("disabled");

        let listing = contributed_tools(&registry.scan().expect("a scan"));
        assert!(listing.tools.is_empty());
        assert!(listing.failures.is_empty());
    }

    #[test]
    fn an_unreadable_tool_schema_hides_one_plugin_and_no_others() {
        let root = scratch("tools-broken");
        let user = root.join("user");
        let broken = install_with_tool(&user, "com.example.bad", "cut_silence", TOOL_SCHEMA);
        std::fs::remove_file(broken.join("cut_silence.json")).expect("the schema is removed");
        install_with_tool(&user, "com.example.good", "tint", TOOL_SCHEMA);

        let listing = contributed_tools(&registry(&root).scan().expect("a scan"));
        assert_eq!(listing.tools.len(), 1);
        assert_eq!(listing.tools[0].plugin, "com.example.good");
        assert_eq!(listing.failures.len(), 1);
        assert_eq!(listing.failures[0].id.as_str(), "com.example.bad");
        assert_eq!(listing.failures[0].code, "plugin.tool_schema_unreadable");
    }

    #[test]
    fn plugin_tools_is_served_over_the_command_api() {
        let root = scratch("tools-method");
        install_with_tool(
            &root.join("user"),
            "com.example.one",
            "cut_silence",
            TOOL_SCHEMA,
        );
        let engine = Engine::spawn(Project::new("Tools")).expect("an engine");
        let mut dispatcher = Dispatcher::new(engine.handle().clone());
        register_methods(&mut dispatcher, Arc::new(registry(&root))).expect("the methods");

        let listing = dispatcher
            .invoke(PLUGIN_TOOLS, None)
            .expect("plugin.tools answers");
        assert_eq!(listing["tools"][0]["name"], "com_example_one_cut_silence");
        assert_eq!(listing["tools"][0]["plugin"], "com.example.one");
        assert_eq!(listing["tools"][0]["tool"], "cut_silence");
        assert!(listing["tools"][0]["input_schema"].is_object());

        engine.shutdown().expect("the engine stops");
    }
}
