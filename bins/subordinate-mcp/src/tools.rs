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

/// The tools this bridge offers, and the method each one calls.
#[derive(Debug, Clone)]
pub struct ToolSet {
    /// The tools, in the schema's order, which is sorted by method name.
    tools: Vec<Tool>,
    /// Tool name to Command API method name.
    methods: BTreeMap<String, String>,
}

impl ToolSet {
    /// The tools of the schema compiled into this build.
    ///
    /// # Errors
    ///
    /// Returns `mcp.schema_invalid` if the compiled document is not a Command
    /// API schema, which would mean the build is broken.
    pub fn committed() -> SubResult<Self> {
        let document: Value = serde_json::from_str(COMMAND_API_SCHEMA).map_err(|error| {
            SubError::new(
                codes::SCHEMA_INVALID,
                "the compiled Command API schema is not JSON",
            )
            .with_cause(&error)
        })?;
        Self::from_schema(&document)
    }

    /// The tools of an exported Command API schema document.
    ///
    /// # Errors
    ///
    /// Returns `mcp.schema_invalid` when the document has no `methods` array,
    /// when a method has no name, when a `$ref` names a definition that is not
    /// there, or when two methods would collide on one tool name.
    pub fn from_schema(document: &Value) -> SubResult<Self> {
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

        let mut tools = Vec::with_capacity(methods.len());
        let mut names = BTreeMap::new();
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
            if let Some(previous) = names.insert(tool_name.clone(), name.clone()) {
                return Err(SubError::new(
                    codes::SCHEMA_INVALID,
                    "two methods map to the same MCP tool name",
                )
                .with_detail("tool", tool_name)
                .with_detail("methods", [previous, name]));
            }
            tools.push(tool);
        }
        Ok(Self {
            tools,
            methods: names,
        })
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
    use super::{COMMAND_API_SCHEMA, ToolSet, tool_name};
    use serde_json::{Value, json};

    /// The committed schema, parsed.
    fn document() -> Value {
        serde_json::from_str(COMMAND_API_SCHEMA).expect("the committed schema is JSON")
    }

    #[test]
    fn every_method_of_the_command_api_becomes_a_tool() {
        let document = document();
        let tools = ToolSet::committed().expect("the committed schema yields tools");
        let methods = document["methods"].as_array().expect("methods");
        assert_eq!(tools.len(), methods.len());
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
