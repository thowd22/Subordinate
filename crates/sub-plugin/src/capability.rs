//! The capability model: what a manifest asks for, what the user approved, and
//! what the sandbox then actually opens.
//!
//! Plugins are sandboxed by default (docs/PLAN.md §6.1). A component starts
//! with no filesystem, no network and no shader compilation, and gets back
//! exactly what its `[capabilities]` block asked for *and* the user approved on
//! install. This module is the three steps of that:
//!
//! 1. **Expansion.** A declared root is written against a variable, never
//!    against a host path: `$PROJECT` is the directory holding the open project
//!    file, `$PLUGIN_DATA` is the plugin's own private directory.
//!    [`PathVars`] holds the values and [`PathVars::expand`] turns
//!    `$PROJECT/footage` into a host path plus the guest path the plugin sees.
//! 2. **Resolution.** [`ResolvedCapabilities::resolve`] turns an approved
//!    [`Capabilities`] block into the [`Preopen`] list a WASI context is built
//!    from, plus the two boolean gates. A root asked for in both `fs_read` and
//!    `fs_write` becomes one read-write preopen, not two.
//! 3. **Gating.** Host functions that reach outside the sandbox — anything the
//!    host does *on behalf of* a plugin, where WASI's own preopen check does
//!    not apply — call [`ResolvedCapabilities::authorize_read`],
//!    [`authorize_write`](ResolvedCapabilities::authorize_write),
//!    [`authorize_network`](ResolvedCapabilities::authorize_network) or
//!    [`authorize_shaders`](ResolvedCapabilities::authorize_shaders) first.
//!    Every refusal is [`codes::CAPABILITY_DENIED`].
//!
//! Approval is the other half. [`ApprovalStore`] records what was granted when
//! a plugin was installed, keyed by plugin id and stamped with a digest of the
//! whole manifest, so a plugin that later rewrites its `plugin.toml` — to ask
//! for the network, or to add a world or an MCP tool — comes back as
//! [`ApprovalStatus::Changed`] and must be approved again before it loads.
//!
//! ```
//! use std::path::Path;
//! use sub_plugin::capability::{Access, PathVars, ResolvedCapabilities};
//! use sub_plugin::manifest::Capabilities;
//!
//! let vars = PathVars::new(Path::new("/home/e/.local/share/subordinate/plugins/x/data"))
//!     .unwrap()
//!     .with_project(Path::new("/projects/doc"))
//!     .unwrap();
//!
//! let capabilities = Capabilities {
//!     fs_read: vec!["$PROJECT".to_owned()],
//!     ..Capabilities::default()
//! };
//! let resolved = ResolvedCapabilities::resolve(&capabilities, &vars).unwrap();
//!
//! // One read-only preopen, mounted at a stable guest path.
//! assert_eq!(resolved.preopens().len(), 1);
//! assert_eq!(resolved.preopens()[0].guest_path(), "/project");
//! assert_eq!(resolved.preopens()[0].access(), Access::ReadOnly);
//!
//! // Reading inside it is allowed; writing it, or reading elsewhere, is not.
//! resolved
//!     .authorize_read(Path::new("/projects/doc/footage/a.mov"))
//!     .unwrap();
//! assert!(resolved.authorize_write(Path::new("/projects/doc/a.mov")).is_err());
//! assert!(resolved.authorize_read(Path::new("/etc/passwd")).is_err());
//! assert!(resolved.authorize_network().is_err());
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use wasmtime_wasi::{FsPerms, WasiCtxBuilder};

use crate::codes;
use crate::manifest::{Capabilities, Manifest, PluginId};

/// The variable a root must start with to mean the open project's directory.
pub const PROJECT_VAR: &str = "$PROJECT";
/// The variable a root must start with to mean the plugin's own data directory.
pub const PLUGIN_DATA_VAR: &str = "$PLUGIN_DATA";
/// Where `$PROJECT` is mounted inside the sandbox.
pub const PROJECT_MOUNT: &str = "/project";
/// Where `$PLUGIN_DATA` is mounted inside the sandbox.
pub const PLUGIN_DATA_MOUNT: &str = "/plugin-data";

/// The version stamped into a serialised [`ApprovalStore`].
pub const APPROVALS_SCHEMA_VERSION: u32 = 1;

/// What a preopened directory may be used for.
///
/// Read-write is the stronger of the two: resolving a root that appears in both
/// `fs_read` and `fs_write` yields one [`Access::ReadWrite`] preopen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// The plugin may open and read files under the root.
    ReadOnly,
    /// The plugin may also create, write and remove them.
    ReadWrite,
}

impl Access {
    /// True when this access permits writing.
    #[must_use]
    pub const fn is_writable(self) -> bool {
        matches!(self, Self::ReadWrite)
    }

    /// The name used in an error detail and in `[capabilities]`.
    #[must_use]
    pub const fn capability_name(self) -> &'static str {
        match self {
            Self::ReadOnly => "fs_read",
            Self::ReadWrite => "fs_write",
        }
    }
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
        })
    }
}

/// One directory the sandbox opens for a plugin: a host path, the path the
/// guest sees it at, and what it may do there.
///
/// This is exactly the argument shape a WASI preopen takes — host directory,
/// guest path, permissions — so a host builds its `WasiCtx` by walking
/// [`ResolvedCapabilities::preopens`] and adding one preopened directory per
/// entry, with directory and file permissions from [`Preopen::access`]
/// (TASK-84).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preopen {
    /// The directory on the host.
    host_path: PathBuf,
    /// Where it appears inside the guest, always slash-separated and absolute.
    guest_path: String,
    /// What may be done there.
    access: Access,
}

impl Preopen {
    /// The host directory to open.
    #[must_use]
    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    /// The guest-visible path the directory is mounted at.
    #[must_use]
    pub fn guest_path(&self) -> &str {
        &self.guest_path
    }

    /// What the plugin may do inside it.
    #[must_use]
    pub const fn access(&self) -> Access {
        self.access
    }
}

/// The values `$PROJECT` and `$PLUGIN_DATA` expand to.
///
/// `$PLUGIN_DATA` always has a value — every installed plugin has a private
/// directory, whether or not anything is open — while `$PROJECT` has one only
/// while a project is open, which is why expanding a `$PROJECT` root without
/// one is [`codes::UNSET_PATH_VARIABLE`] rather than a silent empty grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathVars {
    /// The directory holding the open project file, when one is open.
    project: Option<PathBuf>,
    /// The plugin's own private directory.
    plugin_data: PathBuf,
}

impl PathVars {
    /// Binds `$PLUGIN_DATA` to an absolute directory.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_CAPABILITY_PATH`] when `plugin_data` is not absolute.
    pub fn new(plugin_data: &Path) -> SubResult<Self> {
        Ok(Self {
            project: None,
            plugin_data: absolute_dir(plugin_data, PLUGIN_DATA_VAR)?,
        })
    }

    /// Binds `$PROJECT` to the directory holding the open project file.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_CAPABILITY_PATH`] when `project` is not absolute.
    pub fn with_project(mut self, project: &Path) -> SubResult<Self> {
        self.project = Some(absolute_dir(project, PROJECT_VAR)?);
        Ok(self)
    }

    /// The directory `$PROJECT` expands to, if a project is open.
    #[must_use]
    pub fn project(&self) -> Option<&Path> {
        self.project.as_deref()
    }

    /// The directory `$PLUGIN_DATA` expands to.
    #[must_use]
    pub fn plugin_data(&self) -> &Path {
        &self.plugin_data
    }

    /// Expands one declared root into the host and guest paths it names.
    ///
    /// A root is a variable, optionally followed by slash-separated segments
    /// under it: `$PROJECT`, `$PROJECT/footage`, `$PLUGIN_DATA/cache`. Nothing
    /// else is accepted — an absolute host path would let a manifest reach
    /// outside the sandbox by writing it down, and a `..` segment would let it
    /// climb out of the root it named.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_CAPABILITY_PATH`] when the root does not start with a
    /// variable or has an empty, `.`, `..` or NUL-bearing segment;
    /// [`codes::UNKNOWN_PATH_VARIABLE`] when the variable is neither
    /// `$PROJECT` nor `$PLUGIN_DATA`; [`codes::UNSET_PATH_VARIABLE`] when it is
    /// `$PROJECT` and no project is open.
    pub fn expand(&self, root: &str, access: Access) -> SubResult<Preopen> {
        let invalid = |reason: &str| {
            SubError::new(
                codes::INVALID_CAPABILITY_PATH,
                format!("capability path {root:?} {reason}"),
            )
            .with_detail("path", root.to_owned())
            .with_detail("capability", access.capability_name())
        };

        let normalised = root.replace('\\', "/");
        let mut segments = normalised.split('/');
        let variable = segments.next().unwrap_or_default();
        if !variable.starts_with('$') {
            return Err(invalid(&format!(
                "must start with {PROJECT_VAR} or {PLUGIN_DATA_VAR}"
            )));
        }

        let (base, mount) = match variable {
            PROJECT_VAR => (
                self.project.clone().ok_or_else(|| {
                    SubError::new(
                        codes::UNSET_PATH_VARIABLE,
                        format!("{PROJECT_VAR} has no value because no project is open"),
                    )
                    .with_detail("variable", PROJECT_VAR)
                    .with_detail("path", root.to_owned())
                })?,
                PROJECT_MOUNT,
            ),
            PLUGIN_DATA_VAR => (self.plugin_data.clone(), PLUGIN_DATA_MOUNT),
            other => {
                return Err(SubError::new(
                    codes::UNKNOWN_PATH_VARIABLE,
                    format!("no path variable named {other}"),
                )
                .with_detail("variable", other.to_owned())
                .with_detail("path", root.to_owned()));
            }
        };

        let mut host_path = base;
        let mut guest_path = mount.to_owned();
        for segment in segments {
            if segment.is_empty() || segment == "." || segment == ".." || segment.contains('\0') {
                return Err(invalid("has an empty, relative or NUL-bearing segment"));
            }
            host_path.push(segment);
            guest_path.push('/');
            guest_path.push_str(segment);
        }

        Ok(Preopen {
            host_path,
            guest_path,
            access,
        })
    }
}

/// Checks that a variable's value is an absolute directory path.
fn absolute_dir(path: &Path, variable: &str) -> SubResult<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Err(SubError::new(
            codes::INVALID_CAPABILITY_PATH,
            format!("{variable} must be bound to an absolute path"),
        )
        .with_detail("variable", variable.to_owned())
        .with_detail("path", path.display().to_string()))
    }
}

/// An approved [`Capabilities`] block with its variables expanded: what the
/// sandbox opens, and what every host function gate answers from.
///
/// Resolution is deliberately the *only* way to get one, so a capability that
/// was never approved cannot be enforced into existence, and the empty value —
/// [`ResolvedCapabilities::sandboxed`] — is the default a plugin gets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedCapabilities {
    /// The directories to preopen, deduplicated by guest path.
    preopens: Vec<Preopen>,
    /// Whether outbound network access was granted.
    network: bool,
    /// Whether the plugin's WGSL may be compiled and run.
    shaders: bool,
}

impl ResolvedCapabilities {
    /// The bare sandbox: no directories, no network, no shaders.
    #[must_use]
    pub fn sandboxed() -> Self {
        Self::default()
    }

    /// Expands an approved capability block against `vars`.
    ///
    /// Roots are expanded in declaration order, `fs_read` before `fs_write`,
    /// and a root named by both ends up as one [`Access::ReadWrite`] preopen.
    ///
    /// # Errors
    ///
    /// Whatever [`PathVars::expand`] returns for the first root that does not
    /// expand.
    pub fn resolve(capabilities: &Capabilities, vars: &PathVars) -> SubResult<Self> {
        let mut by_guest_path: BTreeMap<String, Preopen> = BTreeMap::new();
        for (roots, access) in [
            (&capabilities.fs_read, Access::ReadOnly),
            (&capabilities.fs_write, Access::ReadWrite),
        ] {
            for root in roots {
                let preopen = vars.expand(root, access)?;
                by_guest_path
                    .entry(preopen.guest_path.clone())
                    .and_modify(|existing| {
                        if access.is_writable() {
                            existing.access = Access::ReadWrite;
                        }
                    })
                    .or_insert(preopen);
            }
        }

        Ok(Self {
            preopens: by_guest_path.into_values().collect(),
            network: capabilities.network,
            shaders: capabilities.shaders,
        })
    }

    /// The directories a WASI context should preopen, ordered by guest path.
    #[must_use]
    pub fn preopens(&self) -> &[Preopen] {
        &self.preopens
    }

    /// Whether outbound network access was granted.
    #[must_use]
    pub const fn network(&self) -> bool {
        self.network
    }

    /// Whether the plugin's shaders may be compiled and run.
    #[must_use]
    pub const fn shaders(&self) -> bool {
        self.shaders
    }

    /// Allows a host-side read of `path` on the plugin's behalf.
    ///
    /// # Errors
    ///
    /// [`codes::CAPABILITY_DENIED`] when no granted root contains `path`.
    pub fn authorize_read(&self, path: &Path) -> SubResult<()> {
        self.authorize_path(path, Access::ReadOnly)
    }

    /// Allows a host-side write to `path` on the plugin's behalf.
    ///
    /// # Errors
    ///
    /// [`codes::CAPABILITY_DENIED`] when no granted writable root contains
    /// `path`.
    pub fn authorize_write(&self, path: &Path) -> SubResult<()> {
        self.authorize_path(path, Access::ReadWrite)
    }

    /// Allows the plugin to open a network connection.
    ///
    /// # Errors
    ///
    /// [`codes::CAPABILITY_DENIED`] when `network` was not granted.
    pub fn authorize_network(&self) -> SubResult<()> {
        if self.network {
            Ok(())
        } else {
            Err(denied("network", None))
        }
    }

    /// Allows the host to compile and run the plugin's WGSL.
    ///
    /// # Errors
    ///
    /// [`codes::CAPABILITY_DENIED`] when `shaders` was not granted.
    pub fn authorize_shaders(&self) -> SubResult<()> {
        if self.shaders {
            Ok(())
        } else {
            Err(denied("shaders", None))
        }
    }

    /// Configures a WASI context with exactly what was granted.
    ///
    /// Every preopen becomes one `preopened_dir` at its guest path, with
    /// [`FsPerms::ReadOnly`] or [`FsPerms::ReadWrite`] from its
    /// [`Access`]; the network is inherited only when `network` was granted,
    /// and a builder that is handed a [`ResolvedCapabilities::sandboxed`] grant
    /// is left as it came — no directories, no sockets, no name lookup. Shaders
    /// are not a WASI capability: the host gates those itself through
    /// [`Self::authorize_shaders`] before it compiles a plugin's WGSL.
    ///
    /// # Errors
    ///
    /// [`codes::PREOPEN_FAILED`] when a granted directory cannot be opened —
    /// it was deleted, or was never created.
    pub fn apply_to_wasi(&self, builder: &mut WasiCtxBuilder) -> SubResult<()> {
        for preopen in &self.preopens {
            let perms = if preopen.access.is_writable() {
                FsPerms::ReadWrite
            } else {
                FsPerms::ReadOnly
            };
            builder
                .preopened_dir(&preopen.host_path, &preopen.guest_path, perms)
                .map_err(|err| {
                    SubError::wrap(
                        codes::PREOPEN_FAILED,
                        "could not open a granted directory for the plugin sandbox",
                        err.as_ref(),
                    )
                    .with_detail("path", preopen.host_path.display().to_string())
                    .with_detail("guest_path", preopen.guest_path.clone())
                    .with_detail("capability", preopen.access.capability_name())
                })?;
        }
        if self.network {
            builder.inherit_network().allow_ip_name_lookup(true);
        }
        Ok(())
    }

    /// The shared half of [`Self::authorize_read`] and
    /// [`Self::authorize_write`].
    fn authorize_path(&self, path: &Path, wanted: Access) -> SubResult<()> {
        let capability = wanted.capability_name();
        let normalised = lexically_normalise(path).ok_or_else(|| {
            denied(capability, Some(path)).with_detail("reason", "path is not absolute")
        })?;
        let allowed = self.preopens.iter().any(|preopen| {
            (!wanted.is_writable() || preopen.access.is_writable())
                && normalised.starts_with(&preopen.host_path)
        });
        if allowed {
            Ok(())
        } else {
            Err(denied(capability, Some(&normalised)))
        }
    }
}

/// Builds the one refusal this module returns.
fn denied(capability: &str, path: Option<&Path>) -> SubError {
    let error = SubError::new(
        codes::CAPABILITY_DENIED,
        format!("the plugin was not granted the {capability} capability"),
    )
    .with_detail("capability", capability.to_owned());
    match path {
        Some(path) => error.with_detail("path", path.display().to_string()),
        None => error,
    }
}

/// Resolves `.` and `..` without touching the filesystem, returning `None` for
/// a path that is not absolute or that climbs above its root.
///
/// Symlinks are deliberately not followed: this gate runs before a file need
/// exist, and WASI's own preopen resolution is what stops a symlink escape once
/// the guest holds a directory handle.
fn lexically_normalise(path: &Path) -> Option<PathBuf> {
    let mut normalised = PathBuf::new();
    let mut depth = 0_usize;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalised.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                depth = depth.checked_sub(1)?;
                normalised.pop();
            }
            Component::Normal(segment) => {
                depth += 1;
                normalised.push(segment);
            }
        }
    }
    path.is_absolute().then_some(normalised)
}

/// A digest of a whole `plugin.toml`, as the approval record stamps it.
///
/// It covers the parsed manifest rather than the file's bytes, so reformatting
/// or recommenting a manifest does not force a re-approval while any change to
/// what it declares does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ManifestDigest([u8; 32]);

impl ManifestDigest {
    /// Digests a parsed manifest.
    #[must_use]
    pub fn of(manifest: &Manifest) -> Self {
        // serde_json cannot fail on a Manifest: it is a tree of strings,
        // booleans and maps with string keys.
        let canonical =
            serde_json::to_vec(manifest).unwrap_or_else(|err| unreachable!("manifest: {err}"));
        Self(*blake3::hash(&canonical).as_bytes())
    }

    /// Parses the 64-character lowercase hex form.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_DIGEST`] when `text` is not 64 hex digits.
    pub fn parse(text: &str) -> SubResult<Self> {
        blake3::Hash::from_hex(text)
            .map(|hash| Self(*hash.as_bytes()))
            .map_err(|err| {
                SubError::wrap(
                    codes::INVALID_DIGEST,
                    "a manifest digest must be 64 hex digits",
                    &err,
                )
                .with_detail("digest", text.to_owned())
            })
    }
}

impl fmt::Display for ManifestDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(blake3::Hash::from_bytes(self.0).to_hex().as_str())
    }
}

impl From<ManifestDigest> for String {
    fn from(digest: ManifestDigest) -> Self {
        digest.to_string()
    }
}

impl TryFrom<String> for ManifestDigest {
    type Error = SubError;

    fn try_from(text: String) -> SubResult<Self> {
        Self::parse(&text)
    }
}

/// What the user approved for one plugin when it was installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    /// The manifest the approval was given for.
    pub manifest_digest: ManifestDigest,
    /// The plugin version at that moment, for the prompt the next install
    /// shows.
    pub version: String,
    /// The capabilities that were granted. Enforcement resolves these, never
    /// the manifest's current block.
    pub capabilities: Capabilities,
}

/// Why a plugin may not load with the capabilities it asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalStatus {
    /// The manifest is byte-for-byte what was approved; the recorded
    /// capabilities may be resolved and enforced.
    Approved,
    /// Nothing was ever approved for this plugin id.
    NotApproved,
    /// A different manifest was approved. `capabilities_changed` says whether
    /// the change touched what the sandbox opens, which is what an install
    /// prompt highlights.
    Changed {
        /// What was approved before.
        approved: Box<Approval>,
        /// Whether the `[capabilities]` block itself differs.
        capabilities_changed: bool,
    },
}

impl ApprovalStatus {
    /// True only for [`ApprovalStatus::Approved`].
    #[must_use]
    pub const fn is_approved(&self) -> bool {
        matches!(self, Self::Approved)
    }
}

/// The approvals recorded at install time, one per plugin id.
///
/// The store is the record of a decision a person made, so nothing here ever
/// approves on a plugin's say-so: [`ApprovalStore::approve`] is called by the
/// install flow after the prompt, and [`ApprovalStore::authorize`] is the load
/// path, which resolves the *recorded* capabilities and refuses outright when
/// the manifest has changed underneath them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalStore {
    /// The format of this file, so a later host can migrate it.
    schema_version: u32,
    /// Approvals by plugin id.
    approvals: BTreeMap<String, Approval>,
}

impl Default for ApprovalStore {
    fn default() -> Self {
        Self {
            schema_version: APPROVALS_SCHEMA_VERSION,
            approvals: BTreeMap::new(),
        }
    }
}

impl ApprovalStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the user's approval of `manifest` exactly as it stands.
    ///
    /// Returns the approval it replaced, if the plugin was already installed.
    pub fn approve(&mut self, manifest: &Manifest) -> Option<Approval> {
        self.approvals.insert(
            manifest.plugin.id.as_str().to_owned(),
            Approval {
                manifest_digest: ManifestDigest::of(manifest),
                version: manifest.plugin.version.to_string(),
                capabilities: manifest.capabilities.clone(),
            },
        )
    }

    /// Forgets a plugin's approval, as uninstalling it does.
    pub fn revoke(&mut self, id: &PluginId) -> Option<Approval> {
        self.approvals.remove(id.as_str())
    }

    /// What was approved for `id`, if anything.
    #[must_use]
    pub fn approval(&self, id: &PluginId) -> Option<&Approval> {
        self.approvals.get(id.as_str())
    }

    /// Plugin ids with a recorded approval, in id order.
    pub fn plugin_ids(&self) -> impl Iterator<Item = &str> {
        self.approvals.keys().map(String::as_str)
    }

    /// Whether `manifest` may load on what was approved for it.
    #[must_use]
    pub fn status(&self, manifest: &Manifest) -> ApprovalStatus {
        let Some(approved) = self.approvals.get(manifest.plugin.id.as_str()) else {
            return ApprovalStatus::NotApproved;
        };
        if approved.manifest_digest == ManifestDigest::of(manifest) {
            ApprovalStatus::Approved
        } else {
            ApprovalStatus::Changed {
                capabilities_changed: approved.capabilities != manifest.capabilities,
                approved: Box::new(approved.clone()),
            }
        }
    }

    /// Resolves what `manifest` may do, or refuses to load it.
    ///
    /// This is the whole enforcement path in one call: an unapproved or changed
    /// manifest never reaches [`ResolvedCapabilities`], and an approved one is
    /// resolved from the recorded block, so editing `plugin.toml` after the
    /// fact cannot widen a grant.
    ///
    /// # Errors
    ///
    /// [`codes::NOT_APPROVED`] when nothing was approved for the plugin;
    /// [`codes::APPROVAL_STALE`] when the manifest has changed since, with a
    /// `capabilities_changed` detail; or whatever
    /// [`ResolvedCapabilities::resolve`] returns.
    pub fn authorize(
        &self,
        manifest: &Manifest,
        vars: &PathVars,
    ) -> SubResult<ResolvedCapabilities> {
        let id = manifest.plugin.id.as_str().to_owned();
        match self.status(manifest) {
            ApprovalStatus::Approved => {
                let approved = self
                    .approvals
                    .get(manifest.plugin.id.as_str())
                    .unwrap_or_else(|| unreachable!("Approved implies a recorded approval"));
                ResolvedCapabilities::resolve(&approved.capabilities, vars)
            }
            ApprovalStatus::NotApproved => Err(SubError::new(
                codes::NOT_APPROVED,
                "the plugin's capabilities have not been approved",
            )
            .with_detail("plugin_id", id)),
            ApprovalStatus::Changed {
                approved,
                capabilities_changed,
            } => Err(SubError::new(
                codes::APPROVAL_STALE,
                "the plugin's manifest changed since it was approved; approve it again",
            )
            .with_detail("plugin_id", id)
            .with_detail("approved_version", approved.version.clone())
            .with_detail("requested_version", manifest.plugin.version.to_string())
            .with_detail("capabilities_changed", capabilities_changed)),
        }
    }

    /// Parses a store from the JSON [`Self::to_json`] writes.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_APPROVALS`] when the text is not a store of a schema
    /// version this host understands.
    pub fn from_json(text: &str) -> SubResult<Self> {
        let store: Self = serde_json::from_str(text).map_err(|err| {
            SubError::wrap(
                codes::INVALID_APPROVALS,
                "the recorded plugin approvals are not valid JSON for this host",
                &err,
            )
        })?;
        if store.schema_version == APPROVALS_SCHEMA_VERSION {
            Ok(store)
        } else {
            Err(SubError::new(
                codes::INVALID_APPROVALS,
                "the recorded plugin approvals were written by a different host version",
            )
            .with_detail("schema_version", store.schema_version)
            .with_detail("expected_schema_version", APPROVALS_SCHEMA_VERSION))
        }
    }

    /// Serialises the store as the JSON that is stored beside the installed
    /// plugins.
    ///
    /// # Errors
    ///
    /// Never in practice; the signature keeps the caller honest if the record
    /// shape ever grows a type serde cannot write.
    pub fn to_json(&self) -> SubResult<String> {
        serde_json::to_string_pretty(self).map_err(|err| {
            SubError::wrap(
                codes::INVALID_APPROVALS,
                "the plugin approvals could not be serialised",
                &err,
            )
        })
    }

    /// Reads the store at `path`, treating a missing file as an empty store.
    ///
    /// # Errors
    ///
    /// [`codes::APPROVALS_UNREADABLE`] when the file exists but cannot be
    /// read, or [`codes::INVALID_APPROVALS`] when its contents do not parse.
    pub fn load(path: &Path) -> SubResult<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(err) => Err(SubError::wrap(
                codes::APPROVALS_UNREADABLE,
                "could not read the recorded plugin approvals",
                &err,
            )
            .with_detail("path", path.display().to_string())),
        }
    }

    /// Writes the store to `path`, creating parent directories.
    ///
    /// # Errors
    ///
    /// [`codes::APPROVALS_UNWRITABLE`] when the file cannot be written.
    pub fn save(&self, path: &Path) -> SubResult<()> {
        let unwritable = |err: &std::io::Error| {
            SubError::wrap(
                codes::APPROVALS_UNWRITABLE,
                "could not record the plugin approvals",
                err,
            )
            .with_detail("path", path.display().to_string())
        };
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|err| unwritable(&err))?;
        }
        std::fs::write(path, self.to_json()?).map_err(|err| unwritable(&err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> PathVars {
        PathVars::new(Path::new("/data/plugins/x"))
            .unwrap()
            .with_project(Path::new("/projects/doc"))
            .unwrap()
    }

    fn manifest_with(capabilities: &str) -> Manifest {
        Manifest::parse(&format!(
            "[plugin]\nid = \"com.example.p\"\nname = \"P\"\nversion = \"0.1.0\"\napi = \"0.1\"\nworlds = [\"command\"]\n{capabilities}"
        ))
        .unwrap()
    }

    #[test]
    fn project_and_plugin_data_variables_expand() {
        let vars = vars();
        let read = vars.expand("$PROJECT/footage", Access::ReadOnly).unwrap();
        assert_eq!(read.host_path(), Path::new("/projects/doc/footage"));
        assert_eq!(read.guest_path(), "/project/footage");

        let write = vars.expand("$PLUGIN_DATA", Access::ReadWrite).unwrap();
        assert_eq!(write.host_path(), Path::new("/data/plugins/x"));
        assert_eq!(write.guest_path(), "/plugin-data");
        assert_eq!(write.access(), Access::ReadWrite);
    }

    #[test]
    fn a_windows_style_root_expands_the_same_way() {
        let expanded = vars()
            .expand("$PROJECT\\footage\\a", Access::ReadOnly)
            .unwrap();
        assert_eq!(expanded.guest_path(), "/project/footage/a");
    }

    #[test]
    fn a_host_path_is_not_a_capability_root() {
        let err = vars().expand("/etc", Access::ReadOnly).unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_capability_path");
        assert_eq!(err.details["capability"], "fs_read");
    }

    #[test]
    fn a_parent_segment_is_rejected() {
        let err = vars()
            .expand("$PROJECT/../secrets", Access::ReadOnly)
            .unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_capability_path");
    }

    #[test]
    fn an_unknown_variable_names_itself() {
        let err = vars().expand("$HOME/x", Access::ReadOnly).unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.unknown_path_variable");
        assert_eq!(err.details["variable"], "$HOME");
    }

    #[test]
    fn project_roots_need_an_open_project() {
        let vars = PathVars::new(Path::new("/data/plugins/x")).unwrap();
        let err = vars.expand("$PROJECT", Access::ReadOnly).unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.unset_path_variable");
        assert!(vars.expand("$PLUGIN_DATA", Access::ReadOnly).is_ok());
    }

    #[test]
    fn a_relative_variable_binding_is_rejected() {
        let err = PathVars::new(Path::new("plugins/x")).unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_capability_path");
    }

    #[test]
    fn a_root_named_by_both_lists_becomes_one_read_write_preopen() {
        let capabilities = Capabilities {
            fs_read: vec!["$PLUGIN_DATA".to_owned(), "$PROJECT".to_owned()],
            fs_write: vec!["$PLUGIN_DATA".to_owned()],
            ..Capabilities::default()
        };
        let resolved = ResolvedCapabilities::resolve(&capabilities, &vars()).unwrap();
        assert_eq!(resolved.preopens().len(), 2);
        assert_eq!(resolved.preopens()[0].guest_path(), "/plugin-data");
        assert_eq!(resolved.preopens()[0].access(), Access::ReadWrite);
        assert_eq!(resolved.preopens()[1].guest_path(), "/project");
        assert_eq!(resolved.preopens()[1].access(), Access::ReadOnly);
    }

    #[test]
    fn the_bare_sandbox_grants_nothing() {
        let sandboxed = ResolvedCapabilities::sandboxed();
        assert!(sandboxed.preopens().is_empty());
        assert!(!sandboxed.network());
        assert!(!sandboxed.shaders());
        assert_eq!(
            sandboxed
                .authorize_read(Path::new("/projects/doc/a.mov"))
                .unwrap_err()
                .code
                .as_str(),
            "plugin.capability_denied"
        );
        assert!(sandboxed.authorize_network().is_err());
        assert!(sandboxed.authorize_shaders().is_err());
    }

    #[test]
    fn gating_follows_the_granted_roots() {
        let capabilities = Capabilities {
            fs_read: vec!["$PROJECT".to_owned()],
            fs_write: vec!["$PLUGIN_DATA/cache".to_owned()],
            network: true,
            shaders: true,
        };
        let resolved = ResolvedCapabilities::resolve(&capabilities, &vars()).unwrap();

        resolved
            .authorize_read(Path::new("/projects/doc/a/b.mov"))
            .unwrap();
        resolved
            .authorize_write(Path::new("/data/plugins/x/cache/thumb.png"))
            .unwrap();
        // Write access implies read access to the same root.
        resolved
            .authorize_read(Path::new("/data/plugins/x/cache/thumb.png"))
            .unwrap();
        resolved.authorize_network().unwrap();
        resolved.authorize_shaders().unwrap();

        // Read-only roots are not writable, siblings are not covered, and a
        // traversal is normalised before it is checked.
        assert!(
            resolved
                .authorize_write(Path::new("/projects/doc/a.mov"))
                .is_err()
        );
        assert!(
            resolved
                .authorize_read(Path::new("/data/plugins/x/other"))
                .is_err()
        );
        assert!(
            resolved
                .authorize_read(Path::new("/projects/doc/../../etc/passwd"))
                .is_err()
        );
        assert!(resolved.authorize_read(Path::new("relative/path")).is_err());
    }

    #[test]
    fn approving_records_the_capabilities_and_lets_the_plugin_load() {
        let manifest = manifest_with("[capabilities]\nfs_read = [\"$PROJECT\"]\n");
        let mut store = ApprovalStore::new();
        assert_eq!(store.status(&manifest), ApprovalStatus::NotApproved);
        assert_eq!(
            store
                .authorize(&manifest, &vars())
                .unwrap_err()
                .code
                .as_str(),
            "plugin.not_approved"
        );

        assert!(store.approve(&manifest).is_none());
        assert!(store.status(&manifest).is_approved());
        let approval = store.approval(&manifest.plugin.id).unwrap();
        assert_eq!(approval.capabilities.fs_read, vec!["$PROJECT".to_owned()]);
        assert_eq!(approval.version, "0.1.0");
        assert_eq!(
            store.plugin_ids().collect::<Vec<_>>(),
            vec!["com.example.p"]
        );

        let resolved = store.authorize(&manifest, &vars()).unwrap();
        assert_eq!(resolved.preopens().len(), 1);
    }

    #[test]
    fn a_widened_manifest_needs_approving_again() {
        let approved_manifest = manifest_with("[capabilities]\nfs_read = [\"$PROJECT\"]\n");
        let mut store = ApprovalStore::new();
        store.approve(&approved_manifest);

        let widened = manifest_with("[capabilities]\nfs_read = [\"$PROJECT\"]\nnetwork = true\n");
        let ApprovalStatus::Changed {
            approved,
            capabilities_changed,
        } = store.status(&widened)
        else {
            panic!("a widened manifest must not stay approved");
        };
        assert!(capabilities_changed);
        assert!(!approved.capabilities.network);

        let err = store.authorize(&widened, &vars()).unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.approval_stale");
        assert_eq!(err.details["capabilities_changed"], true);

        // Re-approving takes the new manifest, and the old approval comes back.
        let previous = store.approve(&widened).unwrap();
        assert!(!previous.capabilities.network);
        assert!(store.authorize(&widened, &vars()).unwrap().network());
    }

    #[test]
    fn a_manifest_change_outside_capabilities_still_needs_approving_again() {
        let manifest = manifest_with("");
        let mut store = ApprovalStore::new();
        store.approve(&manifest);

        let renamed = Manifest::parse(
            "[plugin]\nid = \"com.example.p\"\nname = \"P2\"\nversion = \"0.2.0\"\napi = \"0.1\"\nworlds = [\"command\"]\n",
        )
        .unwrap();
        let ApprovalStatus::Changed {
            capabilities_changed,
            ..
        } = store.status(&renamed)
        else {
            panic!("a changed manifest must not stay approved");
        };
        assert!(!capabilities_changed);
    }

    #[test]
    fn revoking_forgets_the_approval() {
        let manifest = manifest_with("");
        let mut store = ApprovalStore::new();
        store.approve(&manifest);
        assert!(store.revoke(&manifest.plugin.id).is_some());
        assert_eq!(store.status(&manifest), ApprovalStatus::NotApproved);
        assert!(store.revoke(&manifest.plugin.id).is_none());
    }

    #[test]
    fn the_store_round_trips_through_json() {
        let manifest = manifest_with("[capabilities]\nfs_write = [\"$PLUGIN_DATA\"]\n");
        let mut store = ApprovalStore::new();
        store.approve(&manifest);
        let json = store.to_json().unwrap();
        assert_eq!(ApprovalStore::from_json(&json).unwrap(), store);
        assert!(json.contains("\"schema_version\": 1"));
    }

    #[test]
    fn a_store_from_another_host_version_is_refused() {
        let err =
            ApprovalStore::from_json(r#"{"schema_version": 99, "approvals": {}}"#).unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_approvals");
        assert_eq!(err.details["schema_version"], 99);
        assert_eq!(
            ApprovalStore::from_json("not json")
                .unwrap_err()
                .code
                .as_str(),
            "plugin.invalid_approvals"
        );
    }

    #[test]
    fn a_digest_round_trips_through_hex() {
        let digest = ManifestDigest::of(&manifest_with(""));
        assert_eq!(ManifestDigest::parse(&digest.to_string()).unwrap(), digest);
        assert_eq!(
            ManifestDigest::parse("nope").unwrap_err().code.as_str(),
            "plugin.invalid_digest"
        );
    }
}
