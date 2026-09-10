//! `plugin.toml`: what a plugin says about itself before any of its code runs.
//!
//! The manifest (docs/PLAN.md §6.3) is the only thing the host reads from an
//! uninstalled plugin, so it carries everything an install decision needs:
//! identity, the interface version it was built against, the WIT worlds it
//! implements, the capabilities it wants granted, and the MCP tools it
//! contributes.
//!
//! ```toml
//! [plugin]
//! id = "com.example.silence-cutter"
//! name = "Silence Cutter"
//! version = "0.1.0"
//! api = "0.1"
//! worlds = ["command", "mcp-tools"]
//!
//! [capabilities]
//! fs_read = ["$PROJECT"]
//! network = false
//!
//! [mcp.tools.cut_silence]
//! description = "Remove silent regions from the selected clips"
//! schema = "schemas/cut_silence.json"
//! ```
//!
//! Parsing is strict: an unknown key is an error rather than a silently
//! ignored line, because a misspelt capability must not read as "not
//! requested". Every failure is a [`SubError`] whose `field` detail is the
//! dotted path of the offending key, so a scaffolding agent can fix the exact
//! line it wrote.
//!
//! ```
//! use sub_plugin::manifest::{Manifest, World};
//!
//! let manifest = Manifest::parse(
//!     r#"
//!     [plugin]
//!     id = "com.example.silence-cutter"
//!     name = "Silence Cutter"
//!     version = "0.1.0"
//!     api = "0.1"
//!     worlds = ["command"]
//!     "#,
//! )
//! .unwrap();
//! assert_eq!(manifest.plugin.id.as_str(), "com.example.silence-cutter");
//! assert!(manifest.declares(World::Command));
//! assert!(!manifest.capabilities.network);
//!
//! let err = Manifest::parse("[plugin]\nid = \"nodots\"\nname = \"N\"\nversion = \"0.1.0\"\napi = \"0.1\"\nworlds = [\"command\"]\n")
//!     .unwrap_err();
//! assert_eq!(err.code.as_str(), "plugin.invalid_manifest");
//! assert_eq!(err.details["field"], "plugin.id");
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use schemars::JsonSchema;
use semver::Version;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};

use crate::codes;

/// The file name a plugin's manifest always has.
pub const MANIFEST_FILE_NAME: &str = "plugin.toml";

/// The plugin interface version this build of the host implements.
///
/// A manifest declares the version it was built against in `plugin.api`; see
/// [`ApiVersion::is_supported_by_host`] for what the host accepts.
pub const HOST_API_VERSION: ApiVersion = ApiVersion { major: 0, minor: 1 };

/// A parsed and validated `plugin.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Identity, interface version and the worlds implemented.
    pub plugin: PluginSection,
    /// What the plugin asks to be allowed to do. Empty when absent.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// The MCP tools the plugin contributes. Empty when absent.
    #[serde(default)]
    pub mcp: McpSection,
}

impl Manifest {
    /// Parses and validates the text of a `plugin.toml`.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_MANIFEST_SYNTAX`] when the text is not the TOML the
    /// manifest shape expects (an unknown key included), with `line` and
    /// `column` details when the parser reports a span;
    /// [`codes::INVALID_MANIFEST`] when it parses but breaks a rule, with the
    /// dotted `field` path of the first problem and every problem in
    /// `fields`.
    pub fn parse(text: &str) -> SubResult<Self> {
        let raw: RawManifest = toml::from_str(text).map_err(|err| {
            let mut error = SubError::wrap(
                codes::INVALID_MANIFEST_SYNTAX,
                format!("{MANIFEST_FILE_NAME} is not valid TOML for a plugin manifest"),
                &err,
            );
            if let Some(span) = err.span() {
                let (line, column) = line_and_column(text, span.start);
                error = error
                    .with_detail("line", line)
                    .with_detail("column", column);
            }
            error
        })?;
        raw.into_manifest()
    }

    /// Reads and validates the `plugin.toml` inside `directory`.
    ///
    /// # Errors
    ///
    /// [`codes::MANIFEST_UNREADABLE`] when the file cannot be read, and
    /// whatever [`Manifest::parse`] returns otherwise.
    pub fn read_dir(directory: impl AsRef<Path>) -> SubResult<Self> {
        Self::read_file(directory.as_ref().join(MANIFEST_FILE_NAME))
    }

    /// Reads and validates a manifest file.
    ///
    /// # Errors
    ///
    /// [`codes::MANIFEST_UNREADABLE`] when `path` cannot be read, and
    /// whatever [`Manifest::parse`] returns otherwise. Both name the offending
    /// file and carry the hint their code implies (see [`crate::errors`]).
    pub fn read_file(path: impl AsRef<Path>) -> SubResult<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|err| {
                SubError::wrap(
                    codes::MANIFEST_UNREADABLE,
                    format!("cannot read the plugin manifest at {}", path.display()),
                    &err,
                )
                .with_detail("path", path.display().to_string())
            })
            .map_err(crate::errors::explain)?;
        Self::parse(&text).map_err(|err| {
            crate::errors::explain(err.with_detail("path", path.display().to_string()))
        })
    }

    /// True when the plugin implements `world`.
    #[must_use]
    pub fn declares(&self, world: World) -> bool {
        self.plugin.worlds.contains(&world)
    }

    /// Checks every rule the field types do not already enforce.
    ///
    /// [`Manifest::parse`] runs this; call it after building a manifest by
    /// hand or after deserializing one from something other than TOML.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_MANIFEST`] listing every broken rule: `field` is the
    /// dotted path of the first one and `fields` is an array of
    /// `{field, problem}` objects, in the order the fields appear in the
    /// manifest.
    pub fn validate(&self) -> SubResult<()> {
        let mut problems = Vec::new();
        validate_name(&self.plugin.name, &mut problems);
        validate_api(self.plugin.api, &mut problems);
        validate_worlds(&self.plugin.worlds, &mut problems);
        validate_description(self.plugin.description.as_deref(), &mut problems);
        self.capabilities.collect_problems(&mut problems);
        self.mcp
            .collect_problems(&self.plugin.worlds, &mut problems);
        into_result(&problems)
    }
}

/// `plugin.toml` as written, before the scalar fields are parsed.
///
/// Deserializing straight into [`Manifest`] would report a malformed id or
/// version as a TOML error somewhere in the file; going through this mirror
/// lets every one of those failures carry its own dotted field path, which is
/// what a scaffolding agent needs to fix the line it wrote.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    plugin: RawPluginSection,
    #[serde(default)]
    capabilities: Capabilities,
    #[serde(default)]
    mcp: McpSection,
}

/// `[plugin]` with its scalars still as strings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPluginSection {
    id: String,
    name: String,
    version: String,
    api: String,
    worlds: Vec<World>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
}

impl RawManifest {
    /// Parses the scalars and applies every rule, collecting the problems of
    /// all of them rather than stopping at the first.
    fn into_manifest(self) -> SubResult<Manifest> {
        let mut problems = Vec::new();
        let plugin = self.plugin;

        let id = PluginId::parse(plugin.id)
            .map_err(|err| Problem::push(&mut problems, "plugin.id", err.to_string()))
            .ok();
        validate_name(&plugin.name, &mut problems);
        let version = Version::parse(&plugin.version)
            .map_err(|err| {
                Problem::push(
                    &mut problems,
                    "plugin.version",
                    format!("must be a semver version such as `0.1.0`: {err}"),
                );
            })
            .ok();
        let api = plugin
            .api
            .parse::<ApiVersion>()
            .map_err(|err| Problem::push(&mut problems, "plugin.api", err.to_string()))
            .ok();
        if let Some(api) = api {
            validate_api(api, &mut problems);
        }
        validate_worlds(&plugin.worlds, &mut problems);
        validate_description(plugin.description.as_deref(), &mut problems);
        self.capabilities.collect_problems(&mut problems);
        self.mcp.collect_problems(&plugin.worlds, &mut problems);
        into_result(&problems)?;

        let (Some(id), Some(version), Some(api)) = (id, version, api) else {
            unreachable!("a scalar that failed to parse recorded a problem")
        };
        Ok(Manifest {
            plugin: PluginSection {
                id,
                name: plugin.name,
                version,
                api,
                worlds: plugin.worlds,
                description: plugin.description,
                authors: plugin.authors,
            },
            capabilities: self.capabilities,
            mcp: self.mcp,
        })
    }
}

/// Turns collected problems into the one [`codes::INVALID_MANIFEST`] error.
fn into_result(problems: &[Problem]) -> SubResult<()> {
    let Some(first) = problems.first() else {
        return Ok(());
    };
    let listed: Vec<serde_json::Value> = problems
        .iter()
        .map(|problem| serde_json::json!({ "field": problem.field, "problem": problem.problem }))
        .collect();
    Err(SubError::new(
        codes::INVALID_MANIFEST,
        format!(
            "{MANIFEST_FILE_NAME} is invalid: {} {}",
            first.field, first.problem
        ),
    )
    .with_detail("field", first.field.clone())
    .with_detail("problem", first.problem.clone())
    .with_detail("fields", listed))
}

/// `plugin.name` is one non-blank line.
fn validate_name(name: &str, problems: &mut Vec<Problem>) {
    if name.trim().is_empty() {
        Problem::push(problems, "plugin.name", "must not be blank");
    } else if name.contains('\n') {
        Problem::push(problems, "plugin.name", "must be a single line");
    }
}

/// `plugin.api` names an interface version this host implements.
fn validate_api(api: ApiVersion, problems: &mut Vec<Problem>) {
    if !api.is_supported_by_host() {
        Problem::push(
            problems,
            "plugin.api",
            format!("declares interface version {api} but this host implements {HOST_API_VERSION}"),
        );
    }
}

/// `plugin.worlds` is a non-empty set.
fn validate_worlds(worlds: &[World], problems: &mut Vec<Problem>) {
    if worlds.is_empty() {
        Problem::push(problems, "plugin.worlds", "must declare at least one world");
    }
    let mut seen = BTreeSet::new();
    for world in worlds {
        if !seen.insert(*world) {
            Problem::push(
                problems,
                "plugin.worlds",
                format!("declares the {world} world twice"),
            );
        }
    }
}

/// `plugin.description`, when present, is not blank.
fn validate_description(description: Option<&str>, problems: &mut Vec<Problem>) {
    if description.is_some_and(|description| description.trim().is_empty()) {
        Problem::push(problems, "plugin.description", "must not be blank");
    }
}

/// One broken rule: where it is and what is wrong with it.
struct Problem {
    /// Dotted path of the offending key, e.g. `mcp.tools.cut_silence.schema`.
    field: String,
    /// One line, lowercase, no trailing period.
    problem: String,
}

impl Problem {
    /// Records a problem at `field`.
    fn push(problems: &mut Vec<Self>, field: impl Into<String>, problem: impl Into<String>) {
        problems.push(Self {
            field: field.into(),
            problem: problem.into(),
        });
    }
}

/// `[plugin]`: who the plugin is and what it implements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginSection {
    /// Reverse-DNS identity, e.g. `com.example.silence-cutter`.
    pub id: PluginId,
    /// Human-readable display name.
    pub name: String,
    /// The plugin's own version, semver.
    #[schemars(with = "String")]
    pub version: Version,
    /// The `subordinate:plugin` interface version it was built against.
    pub api: ApiVersion,
    /// The WIT worlds it implements.
    pub worlds: Vec<World>,
    /// One-sentence description, shown in the plugins panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Free-form author lines, e.g. `Ada <ada@example.com>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
}

/// A reverse-DNS plugin identity such as `com.example.silence-cutter`.
///
/// Two or more dot-separated segments; each starts with a lowercase ASCII
/// letter and continues with lowercase letters, digits or hyphens, and none
/// ends in a hyphen. The id is the key everything else refers to — the
/// install directory, the log tag, the capability grant — so it is validated
/// on the way in rather than trusted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PluginId(String);

impl JsonSchema for PluginId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PluginId".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": r"^[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)+$",
            "maxLength": PluginId::MAX_LEN,
            "description": "Reverse-DNS plugin identity, e.g. \"com.example.silence-cutter\".",
        })
    }
}

impl PluginId {
    /// The longest id accepted, so an id can always be a path segment.
    pub const MAX_LEN: usize = 128;

    /// Parses a reverse-DNS id.
    ///
    /// # Errors
    ///
    /// [`InvalidPluginId`] when `id` is not two or more dot-separated
    /// segments of `[a-z][a-z0-9-]*` that do not end in a hyphen, or when it
    /// is longer than [`PluginId::MAX_LEN`].
    pub fn parse(id: impl Into<String>) -> Result<Self, InvalidPluginId> {
        let id = id.into();
        if id.len() > Self::MAX_LEN || !is_reverse_dns(&id) {
            return Err(InvalidPluginId(id));
        }
        Ok(Self(id))
    }

    /// The id as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// True when `id` is two or more well-formed reverse-DNS segments.
fn is_reverse_dns(id: &str) -> bool {
    let mut segments = 0_usize;
    for segment in id.split('.') {
        let mut bytes = segment.bytes();
        let Some(first) = bytes.next() else {
            return false;
        };
        if !first.is_ascii_lowercase()
            || segment.ends_with('-')
            || !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return false;
        }
        segments += 1;
    }
    segments >= 2
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<PluginId> for String {
    fn from(id: PluginId) -> Self {
        id.0
    }
}

impl TryFrom<String> for PluginId {
    type Error = InvalidPluginId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl FromStr for PluginId {
    type Err = InvalidPluginId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// A string that is not a valid [`PluginId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid plugin id {0:?}: expected a reverse-DNS name of two or more dot-separated \
     segments of [a-z][a-z0-9-], e.g. `com.example.silence-cutter`"
)]
pub struct InvalidPluginId(pub String);

/// A `major.minor` version of the `subordinate:plugin` interface.
///
/// The patch level of a WIT world is not meaningful to a plugin, so the
/// manifest declares only the two components the host shims against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ApiVersion {
    /// Breaking interface generation.
    pub major: u64,
    /// Additive revision within a generation.
    pub minor: u64,
}

impl ApiVersion {
    /// True when this host can run a plugin built against this version.
    ///
    /// Majors must match. Before 1.0 every minor is its own breaking
    /// generation, so the minor must match too; from 1.0 the host runs any
    /// minor up to its own, since a later minor may use interfaces this build
    /// does not export.
    #[must_use]
    pub const fn is_supported_by_host(self) -> bool {
        if self.major != HOST_API_VERSION.major {
            return false;
        }
        if self.major == 0 {
            self.minor == HOST_API_VERSION.minor
        } else {
            self.minor <= HOST_API_VERSION.minor
        }
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for ApiVersion {
    type Err = InvalidApiVersion;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || InvalidApiVersion(value.to_owned());
        let (major, minor) = value.split_once('.').ok_or_else(invalid)?;
        Ok(Self {
            major: major.parse().map_err(|_| invalid())?,
            minor: minor.parse().map_err(|_| invalid())?,
        })
    }
}

impl TryFrom<String> for ApiVersion {
    type Error = InvalidApiVersion;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ApiVersion> for String {
    fn from(version: ApiVersion) -> Self {
        version.to_string()
    }
}

impl JsonSchema for ApiVersion {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ApiVersion".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": r"^\d+\.\d+$",
            "description": "The major.minor version of the subordinate:plugin interface the \
                            plugin was built against, e.g. \"0.1\".",
        })
    }
}

/// A string that is not a valid [`ApiVersion`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid plugin api version {0:?}: expected `major.minor`, e.g. `0.1`")]
pub struct InvalidApiVersion(pub String);

/// A WIT world a plugin can implement (docs/PLAN.md §6.2).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum World {
    /// Parameters plus WGSL shaders, with an optional CPU fallback.
    Effect,
    /// Block-based f32 audio processing under a real-time budget.
    AudioEffect,
    /// Turns a file or URL into media items or a sequence.
    Importer,
    /// Encoder presets or a whole custom export target.
    Exporter,
    /// Background jobs producing metadata.
    Analyzer,
    /// New editing commands built from Command API primitives.
    Command,
    /// A declarative UI panel (post-MVP).
    Panel,
    /// Extra MCP tools the core forwards calls to.
    McpTools,
}

impl World {
    /// Every world, in declaration order.
    pub const ALL: [Self; 8] = [
        Self::Effect,
        Self::AudioEffect,
        Self::Importer,
        Self::Exporter,
        Self::Analyzer,
        Self::Command,
        Self::Panel,
        Self::McpTools,
    ];

    /// The name used in `plugin.worlds` and in the WIT world itself.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Effect => "effect",
            Self::AudioEffect => "audio-effect",
            Self::Importer => "importer",
            Self::Exporter => "exporter",
            Self::Analyzer => "analyzer",
            Self::Command => "command",
            Self::Panel => "panel",
            Self::McpTools => "mcp-tools",
        }
    }
}

impl fmt::Display for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `[capabilities]`: everything the sandbox opens only on request.
///
/// The manifest only *declares* these. Granting them is the install-time
/// approval step and enforcement is the host's, both in the crate's
/// `capability` module: an approval store records what the user approved, and
/// resolving it expands the roots into the sandbox's WASI preopens. Absent
/// keys mean "not requested", which is why parsing rejects unknown ones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// Roots the plugin may read, as `$PROJECT`-style roots or paths under
    /// them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fs_read: Vec<String>,
    /// Roots the plugin may write.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fs_write: Vec<String>,
    /// Whether the plugin may open network connections.
    #[serde(default)]
    pub network: bool,
    /// Whether the plugin may have its WGSL shaders compiled and run.
    #[serde(default)]
    pub shaders: bool,
}

impl Capabilities {
    /// True when nothing beyond the bare sandbox is requested.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fs_read.is_empty() && self.fs_write.is_empty() && !self.network && !self.shaders
    }

    /// Rules the field types cannot express.
    fn collect_problems(&self, problems: &mut Vec<Problem>) {
        for (key, roots) in [("fs_read", &self.fs_read), ("fs_write", &self.fs_write)] {
            for (index, root) in roots.iter().enumerate() {
                let field = format!("capabilities.{key}[{index}]");
                if root.trim().is_empty() {
                    Problem::push(problems, field, "must not be blank");
                } else if root.contains('\0') {
                    Problem::push(problems, field, "must not contain a NUL byte");
                }
            }
        }
    }
}

/// `[mcp]`: the MCP tools the plugin contributes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpSection {
    /// Tools by name, as they appear to an agent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, McpTool>,
}

impl McpSection {
    /// Rules the field types cannot express, including the cross-check
    /// against `plugin.worlds`.
    fn collect_problems(&self, worlds: &[World], problems: &mut Vec<Problem>) {
        if !self.tools.is_empty() && !worlds.contains(&World::McpTools) {
            Problem::push(
                problems,
                "mcp.tools",
                "declares MCP tools without the mcp-tools world in plugin.worlds",
            );
        }
        for (name, tool) in &self.tools {
            if !is_tool_name(name) {
                Problem::push(
                    problems,
                    format!("mcp.tools.{name}"),
                    "must be a name of [a-z][a-z0-9_]* so an agent can call it",
                );
            }
            tool.collect_problems(name, problems);
        }
    }
}

/// True when `name` is a lowercase snake-case MCP tool name.
fn is_tool_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// One `[mcp.tools.<name>]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpTool {
    /// What the tool does, used verbatim as the MCP tool description.
    pub description: String,
    /// The tool's input JSON Schema, relative to the plugin directory.
    #[schemars(with = "String")]
    pub schema: PathBuf,
}

impl McpTool {
    /// Rules the field types cannot express.
    fn collect_problems(&self, name: &str, problems: &mut Vec<Problem>) {
        if self.description.trim().is_empty() {
            Problem::push(
                problems,
                format!("mcp.tools.{name}.description"),
                "must not be blank",
            );
        }
        let field = format!("mcp.tools.{name}.schema");
        if self.schema.as_os_str().is_empty() {
            Problem::push(problems, field, "must not be blank");
        } else if !is_contained_relative_path(&self.schema) {
            Problem::push(
                problems,
                field,
                "must be a relative path inside the plugin directory",
            );
        }
    }
}

/// True when `path` is relative and stays inside its directory.
fn is_contained_relative_path(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

/// The 1-based line and column of the byte at `offset` in `text`.
fn line_and_column(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
    let column = before.rfind('\n').map_or(before.chars().count(), |start| {
        before[start + 1..].chars().count()
    }) + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::{
        ApiVersion, Capabilities, HOST_API_VERSION, Manifest, McpTool, PluginId, World,
        is_contained_relative_path, line_and_column,
    };

    /// The manifest from docs/PLAN.md §6.3, verbatim.
    const PLAN_EXAMPLE: &str = r#"
[plugin]
id = "com.example.silence-cutter"
name = "Silence Cutter"
version = "0.1.0"
api = "0.1"
worlds = ["command", "mcp-tools"]

[capabilities]
fs_read = ["$PROJECT"]
network = false

[mcp.tools.cut_silence]
description = "Remove silent regions from the selected clips"
schema = "schemas/cut_silence.json"
"#;

    /// A minimal manifest whose `[plugin]` line for `key` is replaced by
    /// `line`; `description` is not one of the defaults, so it is appended.
    fn plugin_toml(key: &str, line: &str) -> String {
        let mut text = String::from("[plugin]\n");
        for default in [
            "id = \"com.example.p\"",
            "name = \"P\"",
            "version = \"0.1.0\"",
            "api = \"0.1\"",
            "worlds = [\"command\"]",
        ] {
            if default.starts_with(&format!("{key} =")) {
                text.push_str(line);
            } else {
                text.push_str(default);
            }
            text.push('\n');
        }
        if !text.contains(line) {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// A minimal valid manifest with `body` appended.
    fn with(body: &str) -> String {
        format!(
            "[plugin]\nid = \"com.example.p\"\nname = \"P\"\nversion = \"0.1.0\"\n\
             api = \"0.1\"\nworlds = [\"command\"]\n{body}"
        )
    }

    #[test]
    fn the_plan_example_parses_field_for_field() {
        let manifest = Manifest::parse(PLAN_EXAMPLE).unwrap();
        assert_eq!(manifest.plugin.id.as_str(), "com.example.silence-cutter");
        assert_eq!(manifest.plugin.name, "Silence Cutter");
        assert_eq!(manifest.plugin.version, semver::Version::new(0, 1, 0));
        assert_eq!(manifest.plugin.api, HOST_API_VERSION);
        assert_eq!(
            manifest.plugin.worlds,
            vec![World::Command, World::McpTools]
        );
        assert_eq!(manifest.capabilities.fs_read, vec!["$PROJECT".to_owned()]);
        assert!(!manifest.capabilities.network);
        assert!(!manifest.capabilities.shaders);

        let tool = &manifest.mcp.tools["cut_silence"];
        assert_eq!(
            tool.description,
            "Remove silent regions from the selected clips"
        );
        assert_eq!(
            tool.schema,
            std::path::Path::new("schemas/cut_silence.json")
        );
    }

    #[test]
    fn the_optional_sections_default_to_nothing_requested() {
        let manifest = Manifest::parse(&with("")).unwrap();
        assert!(manifest.capabilities.is_empty());
        assert!(manifest.mcp.tools.is_empty());
        assert!(manifest.declares(World::Command));
        assert!(!manifest.declares(World::Effect));
    }

    #[test]
    fn an_unknown_key_is_a_syntax_error_with_a_position() {
        let err = Manifest::parse(&with("[capabilities]\nnetwrok = true\n")).unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_manifest_syntax");
        assert_eq!(err.details["line"], 8);
        assert_eq!(err.details["column"], 1);
        assert!(err.cause.is_some());
    }

    #[test]
    fn a_missing_required_key_is_a_syntax_error() {
        let err = Manifest::parse("[plugin]\nid = \"com.example.p\"\n").unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_manifest_syntax");
    }

    /// Every rule reports the dotted path of the key that broke it.
    #[test]
    fn validation_failures_name_their_field() {
        for (body, field) in [
            ("name = \"\"", "plugin.name"),
            ("name = \"a\\nb\"", "plugin.name"),
            ("version = \"1\"", "plugin.version"),
            ("api = \"0.9\"", "plugin.api"),
            ("api = \"nope\"", "plugin.api"),
            ("worlds = []", "plugin.worlds"),
            ("worlds = [\"command\", \"command\"]", "plugin.worlds"),
            ("description = \" \"", "plugin.description"),
        ] {
            let key = body.split(' ').next().unwrap();
            let text = plugin_toml(key, body);
            let err = Manifest::parse(&text).unwrap_err();
            assert_eq!(err.code.as_str(), "plugin.invalid_manifest", "{body}");
            assert_eq!(err.details["field"], field, "{body}");
            assert_eq!(err.details["fields"][0]["field"], field, "{body}");
        }
    }

    #[test]
    fn an_id_must_be_reverse_dns() {
        for bad in [
            "nodots",
            "com.",
            ".com.example",
            "com.Example.p",
            "com.ex-.p",
            "com.1st.p",
        ] {
            assert!(PluginId::parse(bad).is_err(), "{bad} should be rejected");
        }
        for good in ["com.example.p", "dev.subordinate.cut-silence", "io.a.b.c"] {
            assert_eq!(PluginId::parse(good).unwrap().as_str(), good);
        }
        assert!(PluginId::parse(format!("com.{}", "a".repeat(PluginId::MAX_LEN))).is_err());
    }

    #[test]
    fn a_bad_id_is_reported_against_its_own_field() {
        let text = "[plugin]\nid = \"nodots\"\nname = \"P\"\nversion = \"0.1.0\"\n\
                    api = \"0.1\"\nworlds = [\"command\"]\n";
        let err = Manifest::parse(text).unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_manifest");
        assert_eq!(err.details["field"], "plugin.id");
    }

    #[test]
    fn a_bad_version_is_reported_against_its_own_field() {
        let text = "[plugin]\nid = \"com.example.p\"\nname = \"P\"\nversion = \"1\"\n\
                    api = \"0.1\"\nworlds = [\"command\"]\n";
        let err = Manifest::parse(text).unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_manifest");
        assert_eq!(err.details["field"], "plugin.version");
    }

    #[test]
    fn every_problem_is_listed_not_just_the_first() {
        let text = "[plugin]\nid = \"com.example.p\"\nname = \"\"\nversion = \"0.1.0\"\n\
                    api = \"0.1\"\nworlds = []\n";
        let err = Manifest::parse(text).unwrap_err();
        let fields = err.details["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0]["field"], "plugin.name");
        assert_eq!(fields[1]["field"], "plugin.worlds");
        assert_eq!(err.details["field"], "plugin.name");
    }

    #[test]
    fn mcp_tools_need_the_mcp_tools_world() {
        let body = "[mcp.tools.cut_silence]\ndescription = \"d\"\nschema = \"s.json\"\n";
        let err = Manifest::parse(&with(body)).unwrap_err();
        assert_eq!(err.details["field"], "mcp.tools");

        let text = with(body).replace("[\"command\"]", "[\"command\", \"mcp-tools\"]");
        assert!(Manifest::parse(&text).is_ok());
    }

    #[test]
    fn a_tool_schema_must_stay_inside_the_plugin_directory() {
        for (schema, ok) in [
            ("schemas/cut.json", true),
            ("./cut.json", true),
            ("../outside.json", false),
            ("/etc/passwd", false),
            ("", false),
        ] {
            let body =
                format!("[mcp.tools.cut_silence]\ndescription = \"d\"\nschema = \"{schema}\"\n");
            let text = with(&body).replace("[\"command\"]", "[\"command\", \"mcp-tools\"]");
            let parsed = Manifest::parse(&text);
            assert_eq!(parsed.is_ok(), ok, "{schema}");
            if !ok {
                assert_eq!(
                    parsed.unwrap_err().details["field"],
                    "mcp.tools.cut_silence.schema",
                    "{schema}",
                );
            }
        }
    }

    #[test]
    fn a_tool_name_must_be_callable_by_an_agent() {
        let body = "[mcp.tools.\"Cut Silence\"]\ndescription = \"d\"\nschema = \"s.json\"\n";
        let text = with(body).replace("[\"command\"]", "[\"command\", \"mcp-tools\"]");
        let err = Manifest::parse(&text).unwrap_err();
        assert_eq!(err.details["field"], "mcp.tools.Cut Silence");
    }

    #[test]
    fn a_blank_capability_root_is_rejected_with_its_index() {
        let body = "[capabilities]\nfs_read = [\"$PROJECT\", \" \"]\n";
        let err = Manifest::parse(&with(body)).unwrap_err();
        assert_eq!(err.details["field"], "capabilities.fs_read[1]");
    }

    #[test]
    fn api_versions_are_matched_against_the_host() {
        assert!(HOST_API_VERSION.is_supported_by_host());
        assert!(!ApiVersion { major: 0, minor: 2 }.is_supported_by_host());
        assert!(!ApiVersion { major: 1, minor: 0 }.is_supported_by_host());
        assert_eq!("0.1".parse::<ApiVersion>().unwrap(), HOST_API_VERSION);
        assert!("0".parse::<ApiVersion>().is_err());
        assert!("0.x".parse::<ApiVersion>().is_err());
        assert_eq!(HOST_API_VERSION.to_string(), "0.1");
    }

    #[test]
    fn every_world_round_trips_through_its_manifest_name() {
        for world in World::ALL {
            let text = with("").replace("[\"command\"]", &format!("[\"{}\"]", world.as_str()));
            let manifest = Manifest::parse(&text).unwrap();
            assert_eq!(manifest.plugin.worlds, vec![world], "{world}");
        }
        let text = with("").replace("[\"command\"]", "[\"nosuch\"]");
        assert_eq!(
            Manifest::parse(&text).unwrap_err().code.as_str(),
            "plugin.invalid_manifest_syntax",
        );
    }

    #[test]
    fn a_manifest_is_read_from_a_directory() {
        let dir = std::env::temp_dir().join(format!("sub-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(super::MANIFEST_FILE_NAME), PLAN_EXAMPLE).unwrap();
        let manifest = Manifest::read_dir(&dir).unwrap();
        assert_eq!(manifest.plugin.name, "Silence Cutter");

        let missing = dir.join("nowhere");
        let err = Manifest::read_dir(&missing).unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.manifest_unreadable");
        assert!(
            err.details["path"]
                .as_str()
                .unwrap()
                .ends_with("plugin.toml")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_parse_error_from_a_file_carries_the_path() {
        let dir = std::env::temp_dir().join(format!("sub-manifest-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(super::MANIFEST_FILE_NAME);
        std::fs::write(&path, "[plugin]\nid = \"nodots\"\nname = \"P\"\nversion = \"0.1.0\"\napi = \"0.1\"\nworlds = [\"command\"]\n").unwrap();
        let err = Manifest::read_file(&path).unwrap_err();
        assert_eq!(err.details["field"], "plugin.id");
        assert_eq!(err.details["path"], path.display().to_string());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_validated_manifest_serializes_back_to_the_shape_it_came_from() {
        let manifest = Manifest::parse(PLAN_EXAMPLE).unwrap();
        let text = toml::to_string(&manifest).unwrap();
        assert_eq!(Manifest::parse(&text).unwrap(), manifest);
    }

    #[test]
    fn capabilities_report_emptiness() {
        assert!(Capabilities::default().is_empty());
        assert!(
            !Capabilities {
                network: true,
                ..Capabilities::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn positions_and_paths_are_computed_as_documented() {
        assert_eq!(line_and_column("ab\ncd", 0), (1, 1));
        assert_eq!(line_and_column("ab\ncd", 3), (2, 1));
        assert_eq!(line_and_column("ab\ncd", 4), (2, 2));
        assert_eq!(line_and_column("ab", 99), (1, 3));
        assert!(is_contained_relative_path(std::path::Path::new("a/b.json")));
        assert!(!is_contained_relative_path(std::path::Path::new("..")));
    }

    #[test]
    fn a_tool_entry_is_validated_on_its_own() {
        let mut problems = Vec::new();
        McpTool {
            description: "  ".to_owned(),
            schema: "s.json".into(),
        }
        .collect_problems("t", &mut problems);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, "mcp.tools.t.description");
    }
}
