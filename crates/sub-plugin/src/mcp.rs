//! The host side of the `mcp-tools` world: what a plugin may contribute to the
//! agent surface, and the checks a call passes before it reaches the plugin.
//!
//! A plugin declares its tools twice. The manifest (`[mcp.tools.*]`,
//! docs/PLAN.md §6.3) is what the user approved on install, and the component's
//! `tools()` export is what the code actually offers. [`ToolCatalog`] holds the
//! manifest side, [`ToolCatalog::accept_exports`] refuses a component whose
//! exports do not match it, and [`ToolCatalog::validate_arguments`] checks a
//! call's arguments against the declared JSON Schema before the host dispatches
//! it. A plugin therefore never sees arguments its own schema rejects, and
//! never offers a tool the manifest did not declare.
//!
//! Names are namespaced by plugin id so an agent can tell two plugins' tools
//! apart. The MCP name is the plugin id with its dots turned into underscores,
//! then an underscore, then the plugin-local name: `com.example.silence-cutter`
//! plus `cut_silence` is `com_example_silence-cutter_cut_silence`. That is the
//! same dot-to-underscore rule the bridge already uses for Command API methods,
//! and it keeps the name inside the `[A-Za-z0-9_-]` set MCP clients accept.
//!
//! ```
//! use serde_json::json;
//! use sub_plugin::mcp::{ToolCatalog, ToolDeclaration};
//!
//! let declaration = ToolDeclaration::new(
//!     "cut_silence",
//!     "Remove silent regions from the selected clips",
//!     json!({
//!         "type": "object",
//!         "properties": { "threshold_db": { "type": "number" } },
//!         "required": ["threshold_db"],
//!     }),
//! )
//! .unwrap();
//! let catalog = ToolCatalog::new("com.example.silence-cutter", [declaration]).unwrap();
//!
//! let tool = catalog.descriptors()[0].clone();
//! assert_eq!(tool.mcp_name, "com_example_silence-cutter_cut_silence");
//! assert_eq!(catalog.local_name(&tool.mcp_name), Some("cut_silence"));
//!
//! catalog
//!     .validate_arguments("cut_silence", r#"{"threshold_db": -40}"#)
//!     .unwrap();
//! let rejected = catalog
//!     .validate_arguments("cut_silence", r#"{"threshold_db": "quiet"}"#)
//!     .unwrap_err();
//! assert_eq!(rejected.code.as_str(), "plugin.invalid_tool_arguments");
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sub_core::{SubError, SubResult};

use crate::bindings::mcp_tools::subordinate::plugin::mcp as wit_mcp;
use crate::codes;
use crate::manifest::Manifest;

/// One tool a plugin's manifest declares.
///
/// The manifest is the source of truth: the description and schema here are
/// what the bridge publishes and what a call is validated against, whatever the
/// component's own `tools()` export says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDeclaration {
    /// The plugin-local name, e.g. `cut_silence`.
    name: String,
    /// One line of prose for the agent choosing the tool.
    description: String,
    /// The argument schema; always a JSON object.
    schema: Value,
}

impl ToolDeclaration {
    /// Declares a tool.
    ///
    /// # Errors
    ///
    /// Returns `plugin.invalid_tool_name` when `name` is not a lowercase
    /// `[a-z][a-z0-9_]*` identifier, and `plugin.invalid_tool_schema` when
    /// `schema` is not a JSON object or is not a schema the host can compile.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
    ) -> SubResult<Self> {
        let name = name.into();
        if !is_tool_name(&name) {
            return Err(SubError::new(
                codes::INVALID_TOOL_NAME,
                "a tool name must be lowercase letters, digits and underscores, starting with a letter",
            )
            .with_detail("tool", name));
        }
        if !schema.is_object() {
            return Err(SubError::new(
                codes::INVALID_TOOL_SCHEMA,
                "a tool's argument schema must be a JSON object",
            )
            .with_detail("tool", name));
        }
        compile(&name, &schema)?;
        Ok(Self {
            name,
            description: description.into(),
            schema,
        })
    }

    /// Declares a tool whose schema is still JSON text, as the manifest's
    /// schema file holds it.
    ///
    /// # Errors
    ///
    /// As [`ToolDeclaration::new`], plus `plugin.invalid_tool_schema` when
    /// `schema_json` is not JSON at all.
    pub fn from_json(
        name: impl Into<String>,
        description: impl Into<String>,
        schema_json: &str,
    ) -> SubResult<Self> {
        let name = name.into();
        let schema: Value = serde_json::from_str(schema_json).map_err(|error| {
            SubError::new(
                codes::INVALID_TOOL_SCHEMA,
                "a tool's argument schema must be JSON",
            )
            .with_detail("tool", name.clone())
            .with_cause(&error)
        })?;
        Self::new(name, description, schema)
    }

    /// The plugin-local name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The description an agent reads.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The argument schema.
    pub fn schema(&self) -> &Value {
        &self.schema
    }
}

/// One tool as the MCP bridge should publish it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    /// The name an MCP client calls, plugin id prefix included.
    pub mcp_name: String,
    /// The plugin-local name, as `call` takes it.
    pub local_name: String,
    /// The dotted, human-facing name: plugin id, a dot, the local name.
    pub title: String,
    /// The description from the manifest.
    pub description: String,
    /// The argument schema from the manifest.
    pub schema: Value,
}

/// One tool as `plugin.tools` publishes it over the Command API.
///
/// The MCP bridge turns a row of this straight into a tool: `name` is the tool
/// name a client calls, `input_schema` its argument schema, and `plugin` and
/// `tool` are what the bridge sends back when the tool is called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishedTool {
    /// The MCP tool name: the plugin id with dots turned into underscores, an
    /// underscore, then the plugin-local name.
    pub name: String,
    /// The dotted, human-facing name: plugin id, a dot, the local name.
    pub title: String,
    /// The description from the manifest.
    pub description: String,
    /// The plugin that contributes the tool.
    pub plugin: String,
    /// The plugin-local name, as the component's `call` takes it.
    pub tool: String,
    /// The argument schema from the manifest.
    pub input_schema: Value,
}

/// A compiled declaration: the manifest entry plus its ready validator.
struct CompiledTool {
    declaration: ToolDeclaration,
    validator: jsonschema::Validator,
    descriptor: ToolDescriptor,
}

/// Every tool one plugin contributes, keyed by plugin-local name.
///
/// Built once per plugin load from the manifest, then consulted on every
/// listing and every call. Schemas are compiled up front so a call costs no
/// more than a validation pass.
pub struct ToolCatalog {
    plugin_id: String,
    /// The plugin id with dots replaced, the prefix of every MCP name.
    prefix: String,
    tools: BTreeMap<String, CompiledTool>,
}

impl std::fmt::Debug for ToolCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolCatalog")
            .field("plugin_id", &self.plugin_id)
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl ToolCatalog {
    /// Builds the catalogue for `plugin_id` from its manifest declarations.
    ///
    /// # Errors
    ///
    /// Returns `plugin.invalid_plugin_id` when the id is not reverse-DNS,
    /// `plugin.duplicate_tool` when two declarations share a name, and the
    /// schema errors of [`ToolDeclaration::new`] when a schema will not
    /// compile.
    pub fn new(
        plugin_id: impl Into<String>,
        declarations: impl IntoIterator<Item = ToolDeclaration>,
    ) -> SubResult<Self> {
        let plugin_id = plugin_id.into();
        if !is_plugin_id(&plugin_id) {
            return Err(SubError::new(
                codes::INVALID_PLUGIN_ID,
                "a plugin id must be two or more lowercase reverse-DNS segments",
            )
            .with_detail("plugin_id", plugin_id));
        }
        let prefix = plugin_id.replace('.', "_");

        let mut tools = BTreeMap::new();
        for declaration in declarations {
            let validator = compile(&declaration.name, &declaration.schema)?;
            let descriptor = ToolDescriptor {
                mcp_name: format!("{prefix}_{}", declaration.name),
                local_name: declaration.name.clone(),
                title: format!("{plugin_id}.{}", declaration.name),
                description: declaration.description.clone(),
                schema: declaration.schema.clone(),
            };
            let name = declaration.name.clone();
            let existing = tools.insert(
                name.clone(),
                CompiledTool {
                    declaration,
                    validator,
                    descriptor,
                },
            );
            if existing.is_some() {
                return Err(SubError::new(
                    codes::DUPLICATE_TOOL,
                    "a plugin declares the same tool twice",
                )
                .with_detail("plugin_id", plugin_id)
                .with_detail("tool", name));
            }
        }
        Ok(Self {
            plugin_id,
            prefix,
            tools,
        })
    }

    /// Builds the catalogue for an installed plugin straight from its
    /// manifest, reading the schema file each `[mcp.tools.*]` entry names.
    ///
    /// `directory` is the directory holding the plugin's `plugin.toml`; a
    /// tool's `schema` path is relative to it, and the manifest parser has
    /// already refused a path that leaves it. A plugin that declares no tools
    /// gives an empty catalogue rather than an error, so a caller can build one
    /// for every installed plugin without asking first.
    ///
    /// # Errors
    ///
    /// Returns `plugin.tool_schema_unreadable` when a declared schema file
    /// cannot be read, plus the errors of [`ToolCatalog::new`] for an id, a
    /// name or a schema the host will not take.
    pub fn from_manifest(manifest: &Manifest, directory: &Path) -> SubResult<Self> {
        let plugin_id = manifest.plugin.id.as_str().to_owned();
        let mut declarations = Vec::with_capacity(manifest.mcp.tools.len());
        for (name, tool) in &manifest.mcp.tools {
            let path = directory.join(&tool.schema);
            let text = std::fs::read_to_string(&path).map_err(|error| {
                SubError::new(
                    codes::TOOL_SCHEMA_UNREADABLE,
                    "a plugin's declared tool schema file cannot be read",
                )
                .with_detail("plugin_id", plugin_id.clone())
                .with_detail("tool", name.clone())
                .with_detail("path", path.display().to_string())
                .with_cause(&error)
            })?;
            declarations.push(ToolDeclaration::from_json(
                name.clone(),
                tool.description.clone(),
                &text,
            )?);
        }
        Self::new(plugin_id, declarations)
    }

    /// The plugin these tools belong to.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Whether the plugin contributes no tools at all.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// How many tools the plugin contributes.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// The declaration of one plugin-local tool.
    pub fn declaration(&self, tool: &str) -> Option<&ToolDeclaration> {
        self.tools.get(tool).map(|compiled| &compiled.declaration)
    }

    /// Every declaration in this catalogue, in name order.
    ///
    /// A loader that builds the catalogue from a manifest hands these back as
    /// the load's [`crate::dev::PluginArtifacts::tools`], so the host rebuilds
    /// exactly the catalogue the manifest describes rather than a second one
    /// read from disk again.
    pub fn declarations(&self) -> Vec<ToolDeclaration> {
        self.tools
            .values()
            .map(|compiled| compiled.declaration.clone())
            .collect()
    }

    /// Every tool as the bridge should publish it, in name order.
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools
            .values()
            .map(|compiled| compiled.descriptor.clone())
            .collect()
    }

    /// Every tool as the Command API publishes it, in name order.
    ///
    /// This is the wire shape of one row of `plugin.tools`: the same
    /// descriptors, carrying the plugin id, so the MCP bridge can list a
    /// plugin's tools and route a call back without holding a catalogue of its
    /// own.
    pub fn published(&self) -> Vec<PublishedTool> {
        self.tools
            .values()
            .map(|compiled| PublishedTool {
                name: compiled.descriptor.mcp_name.clone(),
                title: compiled.descriptor.title.clone(),
                description: compiled.descriptor.description.clone(),
                plugin: self.plugin_id.clone(),
                tool: compiled.descriptor.local_name.clone(),
                input_schema: compiled.descriptor.schema.clone(),
            })
            .collect()
    }

    /// The plugin-local name behind an MCP name, if this plugin owns it.
    ///
    /// The bridge sees one flat list of tools from every plugin, so this is how
    /// a call is routed back to the plugin that declared it.
    pub fn local_name(&self, mcp_name: &str) -> Option<&str> {
        let local = mcp_name.strip_prefix(&self.prefix)?.strip_prefix('_')?;
        self.tools
            .get_key_value(local)
            .map(|(name, _)| name.as_str())
    }

    /// Checks a component's `tools()` export against the manifest.
    ///
    /// The two must declare the same set of names, and an exported schema must
    /// parse and match the declared one, so that what the user approved on
    /// install is what the plugin runs. Descriptions may differ: the manifest's
    /// is what the bridge publishes.
    ///
    /// # Errors
    ///
    /// Returns `plugin.undeclared_tool` for an exported tool the manifest does
    /// not declare, `plugin.missing_tool` for a declared tool the component
    /// does not export, `plugin.invalid_tool_schema` when an exported schema is
    /// not JSON, and `plugin.tool_schema_mismatch` when it differs from the
    /// declared one.
    pub fn accept_exports(&self, exported: &[wit_mcp::ToolDesc]) -> SubResult<()> {
        let mut seen = BTreeSet::new();
        for tool in exported {
            let Some(compiled) = self.tools.get(&tool.name) else {
                return Err(SubError::new(
                    codes::UNDECLARED_TOOL,
                    "the plugin offers a tool its manifest does not declare",
                )
                .with_detail("plugin_id", self.plugin_id.clone())
                .with_detail("tool", tool.name.clone()));
            };
            let schema: Value = serde_json::from_str(&tool.json_schema).map_err(|error| {
                SubError::new(
                    codes::INVALID_TOOL_SCHEMA,
                    "a tool's argument schema must be JSON",
                )
                .with_detail("plugin_id", self.plugin_id.clone())
                .with_detail("tool", tool.name.clone())
                .with_cause(&error)
            })?;
            if schema != compiled.declaration.schema {
                return Err(SubError::new(
                    codes::TOOL_SCHEMA_MISMATCH,
                    "the plugin's tool schema differs from the one its manifest declares",
                )
                .with_detail("plugin_id", self.plugin_id.clone())
                .with_detail("tool", tool.name.clone())
                .with_detail("declared", compiled.declaration.schema.clone())
                .with_detail("exported", schema));
            }
            seen.insert(tool.name.clone());
        }
        for name in self.tools.keys() {
            if !seen.contains(name) {
                return Err(SubError::new(
                    codes::MISSING_TOOL,
                    "the manifest declares a tool the plugin does not export",
                )
                .with_detail("plugin_id", self.plugin_id.clone())
                .with_detail("tool", name.clone()));
            }
        }
        Ok(())
    }

    /// Validates a call's arguments against the declared schema.
    ///
    /// Called before dispatch, so the plugin's `call` only ever sees arguments
    /// its own schema accepts. Every violation is reported, not just the first,
    /// because an agent that gets one error per round trip spends its turns on
    /// the schema instead of the edit.
    ///
    /// # Errors
    ///
    /// Returns `plugin.undeclared_tool` when the plugin has no such tool and
    /// `plugin.invalid_tool_arguments` when the arguments are not a JSON object
    /// or the schema rejects them. The `violations` detail lists each failure
    /// as a JSON Pointer into the arguments and a message.
    pub fn validate_arguments(&self, tool: &str, args_json: &str) -> SubResult<()> {
        let compiled = self.tools.get(tool).ok_or_else(|| {
            SubError::new(
                codes::UNDECLARED_TOOL,
                "the plugin has no tool by that name",
            )
            .with_detail("plugin_id", self.plugin_id.clone())
            .with_detail("tool", tool.to_owned())
        })?;
        let arguments: Value = serde_json::from_str(args_json).map_err(|error| {
            SubError::new(
                codes::INVALID_TOOL_ARGUMENTS,
                "tool arguments must be a JSON object",
            )
            .with_detail("plugin_id", self.plugin_id.clone())
            .with_detail("tool", tool.to_owned())
            .with_cause(&error)
        })?;
        if !arguments.is_object() {
            return Err(SubError::new(
                codes::INVALID_TOOL_ARGUMENTS,
                "tool arguments must be a JSON object",
            )
            .with_detail("plugin_id", self.plugin_id.clone())
            .with_detail("tool", tool.to_owned()));
        }

        let violations: Vec<Value> = compiled
            .validator
            .iter_errors(&arguments)
            .map(|error| {
                json!({
                    "pointer": error.instance_path().to_string(),
                    "message": error.to_string(),
                })
            })
            .collect();
        if violations.is_empty() {
            return Ok(());
        }
        Err(SubError::new(
            codes::INVALID_TOOL_ARGUMENTS,
            "the arguments do not match the tool's schema",
        )
        .with_detail("plugin_id", self.plugin_id.clone())
        .with_detail("tool", tool.to_owned())
        .with_detail("violations", violations))
    }
}

/// Compiles one schema, blaming `tool` if it will not compile.
fn compile(tool: &str, schema: &Value) -> SubResult<jsonschema::Validator> {
    jsonschema::validator_for(schema).map_err(|error| {
        SubError::new(
            codes::INVALID_TOOL_SCHEMA,
            "a tool's argument schema is not a JSON Schema the host can compile",
        )
        .with_detail("tool", tool.to_owned())
        .with_detail("reason", error.to_string())
    })
}

/// `[a-z][a-z0-9_]*`: the names MCP accepts, restricted to one obvious spelling.
fn is_tool_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

/// Two or more `[a-z][a-z0-9_-]*` segments separated by dots: reverse-DNS, as
/// the manifest's `id` field takes it.
fn is_plugin_id(id: &str) -> bool {
    let mut segments = 0;
    for segment in id.split('.') {
        segments += 1;
        let mut characters = segment.chars();
        let first_is_letter = characters
            .next()
            .is_some_and(|first| first.is_ascii_lowercase());
        let rest_is_clean = characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '_'
                || character == '-'
        });
        if !first_is_letter || !rest_is_clean {
            return false;
        }
    }
    segments >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch plugin directory holding a manifest and its schema files.
    fn plugin_directory(
        name: &str,
        manifest: &str,
        schemas: &[(&str, &str)],
    ) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("subordinate-mcp-catalog-{name}"));
        std::fs::remove_dir_all(&directory).ok();
        std::fs::create_dir_all(&directory).expect("a plugin directory");
        std::fs::write(directory.join("plugin.toml"), manifest).expect("a manifest");
        for (file, text) in schemas {
            std::fs::write(directory.join(file), text).expect("a schema file");
        }
        directory
    }

    /// A manifest declaring one tool whose schema lives in `schema.json`.
    const MANIFEST: &str = "[plugin]\nid = \"com.example.silence-cutter\"\nname = \"Cutter\"\n\
                            version = \"0.1.0\"\napi = \"0.1\"\n\
                            worlds = [\"command\", \"mcp-tools\"]\n\n\
                            [mcp.tools.cut_silence]\ndescription = \"Cut the quiet bits\"\n\
                            schema = \"schema.json\"\n";

    #[test]
    fn a_catalogue_is_built_from_a_manifest_and_its_schema_files() {
        let schema = r#"{"type":"object","properties":{"threshold_db":{"type":"number"}}}"#;
        let directory = plugin_directory("built", MANIFEST, &[("schema.json", schema)]);
        let manifest = Manifest::parse(MANIFEST).expect("a manifest");

        let catalog = ToolCatalog::from_manifest(&manifest, &directory).expect("a catalogue");
        assert_eq!(catalog.plugin_id(), "com.example.silence-cutter");
        assert_eq!(catalog.len(), 1);
        let declaration = catalog.declaration("cut_silence").expect("the declaration");
        assert_eq!(declaration.description(), "Cut the quiet bits");
        assert_eq!(
            declaration.schema(),
            &serde_json::from_str::<Value>(schema).expect("the schema")
        );

        let published = catalog.published();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].name, "com_example_silence-cutter_cut_silence");
        assert_eq!(published[0].title, "com.example.silence-cutter.cut_silence");
        assert_eq!(published[0].plugin, "com.example.silence-cutter");
        assert_eq!(published[0].tool, "cut_silence");
        assert_eq!(published[0].description, "Cut the quiet bits");
        assert!(published[0].input_schema.is_object());
    }

    #[test]
    fn a_missing_schema_file_is_reported_against_the_tool() {
        let directory = plugin_directory("missing", MANIFEST, &[]);
        let manifest = Manifest::parse(MANIFEST).expect("a manifest");

        let error = ToolCatalog::from_manifest(&manifest, &directory).expect_err("no schema file");
        assert_eq!(error.code.as_str(), "plugin.tool_schema_unreadable");
        assert_eq!(error.details["tool"], "cut_silence");
        assert_eq!(error.details["plugin_id"], "com.example.silence-cutter");
    }

    #[test]
    fn a_plugin_with_no_tools_gives_an_empty_catalogue() {
        let text = "[plugin]\nid = \"com.example.plain\"\nname = \"Plain\"\n\
                    version = \"0.1.0\"\napi = \"0.1\"\nworlds = [\"command\"]\n";
        let directory = plugin_directory("plain", text, &[]);
        let manifest = Manifest::parse(text).expect("a manifest");

        let catalog = ToolCatalog::from_manifest(&manifest, &directory).expect("a catalogue");
        assert!(catalog.is_empty());
        assert!(catalog.published().is_empty());
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "threshold_db": { "type": "number" },
                "pad_frames": { "type": "integer", "minimum": 0 },
            },
            "required": ["threshold_db"],
            "additionalProperties": false,
        })
    }

    fn declaration() -> ToolDeclaration {
        ToolDeclaration::new("cut_silence", "Remove silent regions", schema()).unwrap()
    }

    fn catalog() -> ToolCatalog {
        ToolCatalog::new("com.example.silence-cutter", [declaration()]).unwrap()
    }

    #[test]
    fn a_tool_name_must_be_a_lowercase_identifier() {
        for bad in ["Cut", "1cut", "cut-silence", "cut.silence", ""] {
            let error = ToolDeclaration::new(bad, "", schema()).unwrap_err();
            assert_eq!(error.code.as_str(), "plugin.invalid_tool_name");
        }
        assert!(ToolDeclaration::new("cut_silence2", "", schema()).is_ok());
    }

    #[test]
    fn a_schema_must_be_an_object_that_compiles() {
        let error = ToolDeclaration::new("cut", "", json!([1, 2])).unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.invalid_tool_schema");

        let error = ToolDeclaration::new("cut", "", json!({ "type": "nonsense" })).unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.invalid_tool_schema");

        let error = ToolDeclaration::from_json("cut", "", "{ not json").unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.invalid_tool_schema");
        assert!(ToolDeclaration::from_json("cut", "", r#"{"type":"object"}"#).is_ok());
    }

    #[test]
    fn a_plugin_id_must_be_reverse_dns() {
        for bad in ["single", "Com.Example.Tool", "com..tool", "com.9lives", ""] {
            let error = ToolCatalog::new(bad, []).unwrap_err();
            assert_eq!(error.code.as_str(), "plugin.invalid_plugin_id");
        }
        assert!(ToolCatalog::new("com.example.silence-cutter", []).is_ok());
    }

    #[test]
    fn a_tool_declared_twice_is_refused() {
        let error =
            ToolCatalog::new("com.example.tool", [declaration(), declaration()]).unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.duplicate_tool");
    }

    #[test]
    fn mcp_names_carry_the_plugin_id_and_map_back() {
        let catalog = catalog();
        let descriptors = catalog.descriptors();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(catalog.len(), 1);
        assert!(!catalog.is_empty());

        let tool = &descriptors[0];
        assert_eq!(tool.mcp_name, "com_example_silence-cutter_cut_silence");
        assert_eq!(tool.title, "com.example.silence-cutter.cut_silence");
        assert_eq!(tool.description, "Remove silent regions");
        assert_eq!(tool.schema, schema());
        assert!(
            tool.mcp_name
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '_'
                    || character == '-')
        );

        assert_eq!(catalog.local_name(&tool.mcp_name), Some("cut_silence"));
        assert_eq!(catalog.local_name("cut_silence"), None);
        assert_eq!(catalog.local_name("com_example_other_cut_silence"), None);
        assert_eq!(
            catalog.local_name("com_example_silence-cutter_unknown"),
            None
        );
    }

    #[test]
    fn exports_must_match_the_manifest() {
        let catalog = catalog();
        let matching = wit_mcp::ToolDesc {
            name: "cut_silence".to_owned(),
            description: "whatever the code says".to_owned(),
            json_schema: serde_json::to_string(&schema()).unwrap(),
        };
        catalog
            .accept_exports(std::slice::from_ref(&matching))
            .unwrap();

        let undeclared = wit_mcp::ToolDesc {
            name: "delete_everything".to_owned(),
            ..matching.clone()
        };
        let error = catalog.accept_exports(&[undeclared]).unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.undeclared_tool");

        let error = catalog.accept_exports(&[]).unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.missing_tool");

        let not_json = wit_mcp::ToolDesc {
            json_schema: "{".to_owned(),
            ..matching.clone()
        };
        let error = catalog.accept_exports(&[not_json]).unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.invalid_tool_schema");

        let widened = wit_mcp::ToolDesc {
            json_schema: r#"{"type":"object"}"#.to_owned(),
            ..matching
        };
        let error = catalog.accept_exports(&[widened]).unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.tool_schema_mismatch");
    }

    #[test]
    fn arguments_are_validated_before_the_call() {
        let catalog = catalog();
        catalog
            .validate_arguments("cut_silence", r#"{"threshold_db": -40, "pad_frames": 2}"#)
            .unwrap();

        let error = catalog
            .validate_arguments("cut_silence", r#"{"pad_frames": 2}"#)
            .unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.invalid_tool_arguments");

        let error = catalog
            .validate_arguments("cut_silence", r#"{"threshold_db": -40, "surprise": true}"#)
            .unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.invalid_tool_arguments");

        let error = catalog.validate_arguments("cut_silence", "[]").unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.invalid_tool_arguments");

        let error = catalog
            .validate_arguments("cut_silence", "not json")
            .unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.invalid_tool_arguments");

        let error = catalog
            .validate_arguments("no_such_tool", "{}")
            .unwrap_err();
        assert_eq!(error.code.as_str(), "plugin.undeclared_tool");
    }

    #[test]
    fn every_violation_is_reported_with_a_pointer() {
        let catalog = catalog();
        let error = catalog
            .validate_arguments(
                "cut_silence",
                r#"{"threshold_db": "loud", "pad_frames": -3}"#,
            )
            .unwrap_err();
        let json = error.to_json();
        let violations = json["details"]["violations"].as_array().unwrap().clone();
        assert_eq!(violations.len(), 2, "{violations:?}");
        let pointers: Vec<&str> = violations
            .iter()
            .map(|violation| violation["pointer"].as_str().unwrap())
            .collect();
        assert!(pointers.contains(&"/threshold_db"), "{pointers:?}");
        assert!(pointers.contains(&"/pad_frames"), "{pointers:?}");
        assert_eq!(json["details"]["tool"], "cut_silence");
    }

    #[test]
    fn the_declaration_is_readable_back() {
        let catalog = catalog();
        let declared = catalog.declaration("cut_silence").unwrap();
        assert_eq!(declared.name(), "cut_silence");
        assert_eq!(declared.description(), "Remove silent regions");
        assert_eq!(declared.schema(), &schema());
        assert!(catalog.declaration("missing").is_none());
        assert_eq!(catalog.plugin_id(), "com.example.silence-cutter");
        assert!(format!("{catalog:?}").contains("cut_silence"));
    }
}
