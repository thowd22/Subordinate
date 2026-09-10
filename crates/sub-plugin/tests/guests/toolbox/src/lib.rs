//! A `subordinate:plugin/mcp-tools` plugin that contributes one MCP tool.
//!
//! It is what TASK-96 needs to prove end to end: an agent lists the tools an
//! editor offers, sees this one under the plugin's id, calls it, and the call
//! arrives in the project as an ordinary undoable command. The tool's
//! arguments are exactly `bin.create`'s parameters, so the whole of `call` is
//! to hand them on — which is the point, since a plugin tool is a Command API
//! call with a name an agent can find.

wit_bindgen::generate!({
    path: "../../../../../wit",
    world: "mcp-tools",
});

// The world `use`s `error` and `tool-desc`, so both are already at the crate
// root; the host imports are reached through the interface module.
use crate::subordinate::plugin::command_api;

struct Plugin;

export!(Plugin);

/// The one tool this plugin contributes.
const TOOL: &str = "create_bin";

/// Its argument schema. The manifest declares the same document, and the host
/// refuses to publish a tool whose two schemas differ.
const SCHEMA: &str = r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"#;

impl Guest for Plugin {
    fn tools() -> Vec<ToolDesc> {
        vec![ToolDesc {
            name: TOOL.to_owned(),
            description: "Create a bin in the open project.".to_owned(),
            json_schema: SCHEMA.to_owned(),
        }]
    }

    fn call(name: String, args_json: String) -> Result<String, Error> {
        if name != TOOL {
            return Err(Error {
                code: "plugin.undeclared_tool".to_owned(),
                message: format!("this plugin has no tool called {name}"),
                details: Vec::new(),
            });
        }
        command_api::log(command_api::LogLevel::Info, "toolbox guest creating a bin");
        let Some(project) = command_api::open_projects().into_iter().next() else {
            return Err(Error {
                code: "plugin.no_such_project".to_owned(),
                message: "the host has no project open".to_owned(),
                details: Vec::new(),
            });
        };
        // The tool's arguments are `bin.create`'s parameters, so they go
        // straight on: the host has already validated them against the schema
        // above.
        let applied = command_api::run_command(&project, "bin.create", &args_json)?;
        Ok(format!(r#"{{"created":{applied}}}"#))
    }
}
