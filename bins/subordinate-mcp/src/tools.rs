//! MCP tools, generated from the committed Command API schema.
//!
//! Nothing here is written by hand: `docs/schema/command-api.json` is produced
//! from the Rust command set (`sub_command::schema`), and every tool the bridge
//! offers is one method of that document — same name, same description, same
//! parameter schema. A method added to the Command API is therefore an MCP tool
//! the next time the schema is exported, and an agent that reads a tool
//! description reads the doc comment on the command itself (docs/PLAN.md §7).
//!
//! The one translation is the name. Command API methods are dotted
//! (`clip.trim_in`); MCP clients, Claude Code among them, accept only
//! `[A-Za-z0-9_-]` in a tool name, so each dot becomes an underscore
//! (`clip_trim_in`) and the tool's `title` carries the method name unchanged.
//! [`ToolSet::method`] maps a tool name back to the method to invoke, so the
//! mapping is never guessed at call time.

use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use serde_json::{Map, Value};
use sub_core::{SubError, SubResult};

use crate::codes;

/// The committed Command API schema, compiled in.
///
/// The bridge describes the build it ships with rather than whatever schema
/// happens to be on disk, and `sub_command::schema`'s
/// `committed_schema_is_up_to_date` test keeps the file honest.
pub const COMMAND_API_SCHEMA: &str = include_str!("../../../docs/schema/command-api.json");

/// The committed plugin management schema, compiled in.
///
/// `plugin.list`, `plugin.enable`, `plugin.disable` and `plugin.remove` are
/// served by the plugin host rather than by the engine, so they are exported
/// as their own document (`sub_plugin::registry::schema`). They become tools
/// exactly like the engine's methods, and calling one is the same JSON-RPC
/// round trip to the running editor.
pub const PLUGIN_API_SCHEMA: &str = include_str!("../../../docs/schema/plugin-api.json");

/// The tools this bridge offers, and the method each one calls.
#[derive(Debug, Clone)]
pub struct ToolSet {
    /// The tools, in the schema's order, which is sorted by method name.
    tools: Vec<Tool>,
    /// Tool name to Command API method name.
    methods: BTreeMap<String, String>,
}

impl ToolSet {
    /// The tools of the schemas compiled into this build: the engine's methods
    /// and the plugin host's.
    ///
    /// # Errors
    ///
    /// Returns `mcp.schema_invalid` if either compiled document is not a
    /// Command API schema, which would mean the build is broken.
    pub fn committed() -> SubResult<Self> {
        let mut tools = Self::from_schema(&compiled_in(COMMAND_API_SCHEMA)?)?;
        tools.extend_from_schema(&compiled_in(PLUGIN_API_SCHEMA)?)?;
        Ok(tools)
    }

    /// The tools of an exported Command API schema document.
    ///
    /// # Errors
    ///
    /// Returns `mcp.schema_invalid` when the document has no `methods` array,
    /// when a method has no name, when a `$ref` names a definition that is not
    /// there, or when two methods would collide on one tool name.
    pub fn from_schema(document: &Value) -> SubResult<Self> {
        let mut tools = Self {
            tools: Vec::new(),
            methods: BTreeMap::new(),
        };
        tools.extend_from_schema(document)?;
        Ok(tools)
    }

    /// Adds the tools of another exported schema document, keeping the ones
    /// already there.
    ///
    /// The engine's methods and the plugin host's are exported as separate
    /// documents, so the bridge offers the union of the two.
    ///
    /// # Errors
    ///
    /// The same `mcp.schema_invalid` cases as [`ToolSet::from_schema`],
    /// including a method whose tool name is already taken.
    pub fn extend_from_schema(&mut self, document: &Value) -> SubResult<()> {
        let methods = document
            .get("methods")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                SubError::new(
                    codes::SCHEMA_INVALID,
                    "the Command API schema has no methods array",
                )
            })?;
        let defs = document.get("$defs").and_then(Value::as_object);

        self.tools.reserve(methods.len());
        for method in methods {
            let name = method
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    SubError::new(codes::SCHEMA_INVALID, "a method in the schema has no name")
                })?
                .to_owned();
            let tool_name = tool_name(&name);
            let schema = input_schema(method.get("params"), defs)
                .map_err(|error| error.with_detail("method", name.clone()))?;

            let mut tool = Tool::new_with_raw(
                tool_name.clone(),
                method
                    .get("description")
                    .and_then(Value::as_str)
                    .map(|text| text.to_owned().into()),
                Arc::new(schema),
            );
            tool.title = Some(name.clone());
            if let Some(previous) = self.methods.insert(tool_name.clone(), name.clone()) {
                return Err(SubError::new(
                    codes::SCHEMA_INVALID,
                    "two methods map to the same MCP tool name",
                )
                .with_detail("tool", tool_name)
                .with_detail("methods", [previous, name]));
            }
            self.tools.push(tool);
        }
        Ok(())
    }

    /// Every tool, in schema order.
    #[must_use]
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// The Command API method a tool name calls, or `None` for a name this
    /// bridge does not serve.
    #[must_use]
    pub fn method(&self, tool: &str) -> Option<&str> {
        self.methods.get(tool).map(String::as_str)
    }

    /// How many tools there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the schema produced no tools at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// The Command API method that lists the tools the installed plugins
/// contribute.
///
/// Served by the plugin host beside `plugin.list`, so the bridge asks the
/// running editor rather than reading plugin directories itself.
pub const PLUGIN_TOOLS_METHOD: &str = "plugin.tools";

/// The Command API method a plugin-contributed tool call is forwarded to.
///
/// The bridge routes by name: a tool the compiled-in schemas do not describe is
/// looked up in [`PluginTools`], and its plugin id and plugin-local name are
/// sent with the client's arguments.
pub const PLUGIN_CALL_METHOD: &str = "plugin.call_tool";

/// Where a plugin-contributed tool call goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRoute {
    /// The plugin that contributes the tool.
    pub plugin: String,
    /// The plugin-local tool name, as the plugin's own `call` takes it.
    pub tool: String,
}

/// The tools the installed plugins contribute, as the editor last reported
/// them.
///
/// Unlike [`ToolSet`], this is not compiled in: which plugins are installed and
/// enabled is a property of the editor the bridge is talking to, so the list is
/// fetched from `plugin.tools` and refreshed whenever a client lists tools.
/// Each tool keeps the published name the host gave it — the plugin id with its
/// dots turned into underscores, an underscore, then the plugin-local name — so
/// two plugins' tools never collide and an agent can see which plugin a tool
/// came from (docs/PLAN.md §6.2).
#[derive(Debug, Clone, Default)]
pub struct PluginTools {
    /// The tools, in published-name order.
    tools: Vec<Tool>,
    /// Published tool name to the plugin and plugin-local name behind it.
    routes: BTreeMap<String, PluginRoute>,
    /// The listing this was read from, as text.
    ///
    /// It is what tells one refresh from the next: after a reload, an install
    /// or a plugin switched on or off, a listing whose text differs means the
    /// tools a client holds are stale and `notifications/tools/list_changed`
    /// is owed. Comparing the whole listing rather than the names catches a
    /// tool whose description or argument schema changed under the same name,
    /// which is the ordinary case while a plugin is being developed.
    digest: String,
}

impl PluginTools {
    /// Reads a `plugin.tools` result.
    ///
    /// # Errors
    ///
    /// Returns `mcp.plugin_tools_invalid` when the result is not the listing
    /// the plugin host documents: a `tools` array of objects with a name, a
    /// plugin id, a plugin-local name and an object input schema.
    pub fn from_listing(listing: &Value) -> SubResult<Self> {
        let rows = listing
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                SubError::new(
                    codes::PLUGIN_TOOLS_INVALID,
                    "a plugin tool listing has no tools array",
                )
            })?;
        let mut parsed = Self {
            digest: Value::Array(rows.clone()).to_string(),
            ..Self::default()
        };
        for row in rows {
            let text = |field: &str| -> SubResult<String> {
                row.get(field)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        SubError::new(
                            codes::PLUGIN_TOOLS_INVALID,
                            "a plugin tool is missing a field",
                        )
                        .with_detail("field", field.to_owned())
                    })
            };
            let name = text("name")?;
            let route = PluginRoute {
                plugin: text("plugin")?,
                tool: text("tool")?,
            };
            let schema = row
                .get("input_schema")
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| {
                    SubError::new(
                        codes::PLUGIN_TOOLS_INVALID,
                        "a plugin tool's input schema is not an object",
                    )
                    .with_detail("tool", name.clone())
                })?;

            let mut tool = Tool::new_with_raw(
                name.clone(),
                row.get("description")
                    .and_then(Value::as_str)
                    .map(|description| description.to_owned().into()),
                Arc::new(schema),
            );
            tool.title = row
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| Some(format!("{}.{}", route.plugin, route.tool)));
            if parsed.routes.insert(name.clone(), route).is_some() {
                return Err(SubError::new(
                    codes::PLUGIN_TOOLS_INVALID,
                    "two plugin tools share one published name",
                )
                .with_detail("tool", name));
            }
            parsed.tools.push(tool);
        }
        parsed
            .tools
            .sort_by(|left, right| left.name.cmp(&right.name));
        Ok(parsed)
    }

    /// Every plugin-contributed tool, in published-name order.
    #[must_use]
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// Where a published tool name goes, or `None` for a name no plugin
    /// contributes.
    #[must_use]
    pub fn route(&self, tool: &str) -> Option<&PluginRoute> {
        self.routes.get(tool)
    }

    /// How many plugin-contributed tools there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether no plugin contributes a tool.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Whether `other` is a different listing from this one, and so whether a
    /// client that holds this one needs telling.
    #[must_use]
    pub fn differs_from(&self, other: &Self) -> bool {
        self.digest != other.digest
    }
}

/// Parses one of the schema documents compiled into this build.
fn compiled_in(text: &str) -> SubResult<Value> {
    serde_json::from_str(text).map_err(|error| {
        SubError::new(
            codes::SCHEMA_INVALID,
            "a compiled-in Command API schema is not JSON",
        )
        .with_cause(&error)
    })
}

/// The MCP tool name for a Command API method name.
///
/// Dots are the only character in a method name an MCP client may reject, so
/// they become underscores and nothing else changes.
#[must_use]
pub fn tool_name(method: &str) -> String {
    method.replace('.', "_")
}

/// The tool's input schema: the method's parameter schema, standing alone.
///
/// A method's `params` is a `$ref` into the document's `$defs`, and the
/// definition it names refers on to others, so the referenced object is copied
/// in and the whole `$defs` map travels with it. The refs inside keep pointing
/// at `#/$defs/…`, which now resolves against the tool schema itself.
fn input_schema(
    params: Option<&Value>,
    defs: Option<&Map<String, Value>>,
) -> SubResult<JsonObject> {
    let mut schema = match params {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(object)) => match object.get("$ref").and_then(Value::as_str) {
            Some(reference) => resolve(reference, defs)?,
            None => object.clone(),
        },
        Some(other) => {
            return Err(
                SubError::new(codes::SCHEMA_INVALID, "a method's params is not a schema")
                    .with_detail("params", other.clone()),
            );
        }
    };
    schema
        .entry("type")
        .or_insert_with(|| Value::String("object".to_owned()));
    if let Some(defs) = defs
        && !defs.is_empty()
    {
        schema.insert("$defs".to_owned(), Value::Object(defs.clone()));
    }
    Ok(schema)
}

/// The definition a `#/$defs/Name` reference points at.
fn resolve(reference: &str, defs: Option<&Map<String, Value>>) -> SubResult<JsonObject> {
    let unresolved = |reason: &str| {
        SubError::new(codes::SCHEMA_INVALID, reason.to_owned())
            .with_detail("reference", reference.to_owned())
    };
    let name = reference
        .strip_prefix("#/$defs/")
        .ok_or_else(|| unresolved("a schema reference does not point into $defs"))?;
    defs.and_then(|defs| defs.get(name))
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| unresolved("a schema reference names a definition that is not there"))
}

#[cfg(test)]
mod tests {
    use super::{COMMAND_API_SCHEMA, PLUGIN_API_SCHEMA, PluginTools, ToolSet, tool_name};
    use serde_json::{Value, json};

    /// A `plugin.tools` answer with two plugins' tools in it.
    fn listing() -> Value {
        json!({
            "tools": [
                {
                    "name": "com_example_tint_tint",
                    "title": "com.example.tint.tint",
                    "description": "Tint a clip",
                    "plugin": "com.example.tint",
                    "tool": "tint",
                    "input_schema": { "type": "object" },
                },
                {
                    "name": "com_example_silence-cutter_cut_silence",
                    "title": "com.example.silence-cutter.cut_silence",
                    "description": "Cut the quiet bits",
                    "plugin": "com.example.silence-cutter",
                    "tool": "cut_silence",
                    "input_schema": { "type": "object" },
                },
            ],
        })
    }

    #[test]
    fn plugin_tools_keep_their_prefixed_names_and_route_back_to_the_plugin() {
        let plugins = PluginTools::from_listing(&listing()).expect("a listing");
        assert_eq!(plugins.len(), 2);
        let names: Vec<&str> = plugins
            .tools()
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect();
        assert_eq!(
            names,
            [
                "com_example_silence-cutter_cut_silence",
                "com_example_tint_tint",
            ]
        );
        for tool in plugins.tools() {
            assert!(
                tool.name.chars().all(|character| {
                    character.is_ascii_alphanumeric() || character == '_' || character == '-'
                }),
                "{} is not a portable tool name",
                tool.name,
            );
        }

        let route = plugins
            .route("com_example_tint_tint")
            .expect("the tool routes somewhere");
        assert_eq!(route.plugin, "com.example.tint");
        assert_eq!(route.tool, "tint");
        assert_eq!(
            plugins
                .tools()
                .iter()
                .find(|tool| tool.name == "com_example_tint_tint")
                .and_then(|tool| tool.title.as_deref()),
            Some("com.example.tint.tint"),
        );
        assert!(plugins.route("bin_create").is_none());
    }

    #[test]
    fn an_empty_listing_contributes_nothing() {
        let plugins = PluginTools::from_listing(&json!({ "tools": [] })).expect("a listing");
        assert!(plugins.is_empty());
        assert!(plugins.tools().is_empty());
    }

    #[test]
    fn a_listing_that_is_not_one_is_a_stable_error() {
        for answer in [
            json!({}),
            json!({ "tools": [{ "name": "a_b", "plugin": "a.b" }] }),
            json!({ "tools": [{
                "name": "a_b", "plugin": "a.b", "tool": "b", "input_schema": 7,
            }] }),
        ] {
            let error = PluginTools::from_listing(&answer).expect_err("not a listing");
            assert_eq!(error.code.as_str(), "mcp.plugin_tools_invalid");
        }
    }

    #[test]
    fn plugin_tools_never_collide_with_the_compiled_in_ones() {
        let tools = ToolSet::committed().expect("the committed tool set");
        let plugins = PluginTools::from_listing(&listing()).expect("a listing");
        for tool in plugins.tools() {
            assert!(
                tools.method(tool.name.as_ref()).is_none(),
                "{} shadows a Command API method",
                tool.name,
            );
        }
    }

    /// The committed schema, parsed.
    fn document() -> Value {
        serde_json::from_str(COMMAND_API_SCHEMA).expect("the committed schema is JSON")
    }

    /// The committed plugin management schema, parsed.
    fn plugin_document() -> Value {
        serde_json::from_str(PLUGIN_API_SCHEMA).expect("the committed plugin schema is JSON")
    }

    #[test]
    fn every_method_of_the_command_api_becomes_a_tool() {
        let document = document();
        let tools = ToolSet::committed().expect("the committed schema yields tools");
        let methods = document["methods"].as_array().expect("methods");
        let plugin_methods = plugin_document()["methods"]
            .as_array()
            .expect("plugin methods")
            .len();
        assert_eq!(tools.len(), methods.len() + plugin_methods);
        assert!(!tools.is_empty());

        for method in methods {
            let name = method["name"].as_str().expect("a method name");
            let tool = tools
                .tools()
                .iter()
                .find(|tool| tool.title.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("{name} has a tool"));
            assert_eq!(tool.name, tool_name(name));
            assert_eq!(
                tool.description.as_deref(),
                method["description"].as_str(),
                "{name}",
            );
            assert_eq!(tools.method(&tool.name), Some(name));
        }
    }

    #[test]
    fn tool_names_are_the_method_names_without_dots() {
        assert_eq!(tool_name("clip.trim_in"), "clip_trim_in");
        assert_eq!(tool_name("bin.move_media"), "bin_move_media");
        let tools = ToolSet::committed().expect("tools");
        for tool in tools.tools() {
            assert!(
                tool.name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_'),
                "{} is not a portable tool name",
                tool.name,
            );
        }
        assert_eq!(tools.method("clip_trim_in"), Some("clip.trim_in"));
        assert_eq!(tools.method("clip.trim_in"), None);
    }

    #[test]
    fn a_tool_carries_the_parameter_schema_and_the_definitions_it_needs() {
        let tools = ToolSet::committed().expect("tools");
        let add = tools
            .tools()
            .iter()
            .find(|tool| tool.name == "clip_add")
            .expect("clip.add is a tool");
        let schema = add.input_schema.as_ref();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].get("sequence").is_some());
        // `Clip` is referred to from the parameters, so it travels with them.
        assert!(schema["$defs"].get("Clip").is_some());
    }

    #[test]
    fn the_plugin_management_methods_are_tools_too() {
        let tools = ToolSet::committed().expect("tools");
        for method in [
            "plugin.list",
            "plugin.enable",
            "plugin.disable",
            "plugin.remove",
        ] {
            let name = tool_name(method);
            let tool = tools
                .tools()
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("{method} has a tool"));
            assert_eq!(tool.title.as_deref(), Some(method));
            assert_eq!(tools.method(&name), Some(method));
            assert_eq!(tool.input_schema["type"], "object");
        }
        let enable = tools
            .tools()
            .iter()
            .find(|tool| tool.name == "plugin_enable")
            .expect("plugin.enable is a tool");
        assert!(enable.input_schema["properties"].get("id").is_some());
    }

    #[test]
    fn extending_with_a_document_that_repeats_a_name_is_reported() {
        let mut tools = ToolSet::from_schema(&json!({
            "methods": [{ "name": "plugin.list", "description": "List." }],
        }))
        .expect("one tool");
        let clash = tools
            .extend_from_schema(&json!({
                "methods": [{ "name": "plugin.list", "description": "List again." }],
            }))
            .expect_err("the name is taken");
        assert_eq!(clash.code.as_str(), "mcp.schema_invalid");
        assert_eq!(tools.len(), 1, "the clashing tool was not offered");
    }

    #[test]
    fn a_method_without_parameters_takes_an_empty_object() {
        let tools = ToolSet::from_schema(&json!({
            "methods": [{ "name": "project.revision", "description": "The revision." }],
        }))
        .expect("a schema with no $defs");
        let tool = &tools.tools()[0];
        assert_eq!(tool.name, "project_revision");
        assert_eq!(tool.input_schema["type"], "object");
        assert!(tool.input_schema.get("$defs").is_none());
    }

    #[test]
    fn a_schema_the_bridge_cannot_use_is_reported_with_a_stable_code() {
        let no_methods = ToolSet::from_schema(&json!({})).expect_err("no methods array");
        assert_eq!(no_methods.code.as_str(), "mcp.schema_invalid");

        let dangling = ToolSet::from_schema(&json!({
            "methods": [{ "name": "bin.create", "params": { "$ref": "#/$defs/Absent" } }],
        }))
        .expect_err("a reference to nothing");
        assert_eq!(dangling.code.as_str(), "mcp.schema_invalid");
        assert!(dangling.to_json().to_string().contains("bin.create"));

        let colliding = ToolSet::from_schema(&json!({
            "methods": [{ "name": "a.b" }, { "name": "a_b" }],
        }))
        .expect_err("two methods, one tool name");
        assert_eq!(colliding.code.as_str(), "mcp.schema_invalid");
    }
}
