//! `plugin.new` and `plugin.test`: the two ends of the plugin developer loop,
//! on the Command API.
//!
//! Scaffolding a plugin crate and running the headless test harness over an
//! installed one are `subordinate-cli` subcommands (TASK-90, TASK-91). They are
//! also the first and last steps of the loop an agent runs from a conversation
//! — scaffold, build, install, test (docs/PLAN.md §6.4) — and an agent reaches
//! the editor over MCP, not over a shell it may not have. So both are served as
//! Command API methods too, and the MCP bridge turns them into `plugin_new` and
//! `plugin_test` beside `plugin_install`, `plugin_reload` and `plugin_list`.
//!
//! The *implementations* stay where they are. Generating a crate writes
//! templates that belong to the CLI, and running the harness needs a fixture
//! project the CLI knows how to find, so this module holds the wire types, the
//! method names and the schema, and the host supplies the two handlers. That
//! keeps one description and one parameter schema for each method whichever way
//! it is called: `subordinate-cli plugin new` and `plugin_new` over MCP do the
//! same thing because they run the same code.
//!
//! The `build` step in the middle is deliberately not here. Building a plugin
//! is `cargo build --release --target wasm32-wasip2` in the agent's own
//! terminal; the editor does not run compilers on an agent's behalf.

use std::path::PathBuf;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sub_command::Dispatcher;
use sub_core::{SubError, SubResult};

use crate::manifest::{PluginId, World};

/// `plugin.new`: scaffold a plugin crate for one WIT world.
pub const PLUGIN_NEW: &str = "plugin.new";
/// `plugin.test`: run the headless harness over an installed plugin.
pub const PLUGIN_TEST: &str = "plugin.test";

/// What `plugin.new` describes itself as, in both the dispatcher and the
/// exported schema.
const NEW_DESCRIPTION: &str = "Scaffold a plugin crate for one WIT world, with its manifest, CLAUDE.md, source template \
     and fixture project.";

/// What `plugin.test` describes itself as.
const TEST_DESCRIPTION: &str = "Run the headless test harness over an installed plugin: every check its declared worlds \
     call for, against a fixture project.";

/// The parameters of `plugin.new`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewParams {
    /// The plugin's name, which is also its crate and directory name:
    /// lowercase letters, digits and single hyphens, e.g. `cut-silence`.
    pub name: String,
    /// The WIT world the plugin implements. Not every world has a template;
    /// one that has none is refused with `core.invalid_argument` listing the
    /// ones that do.
    pub world: World,
    /// Where the crate directory goes. The server's working directory by
    /// default, so an agent that wants it somewhere in particular says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub parent: Option<PathBuf>,
    /// The reverse-DNS plugin id, defaulting to `com.example.<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// A local `subordinate-sdk` checkout to depend on by path, instead of the
    /// published version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub sdk_path: Option<PathBuf>,
    /// Whether an existing directory may be written into.
    #[serde(default)]
    pub force: bool,
}

/// The parameters of `plugin.test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestParams {
    /// The installed plugin's reverse-DNS id.
    pub id: PluginId,
    /// A project to run the plugin against, overriding the plugin's own
    /// fixture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub fixture: Option<PathBuf>,
    /// The arguments handed to a command plugin's `run` and to every MCP tool
    /// call. An empty object by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, Value>>")]
    pub arguments: Option<Value>,
}

impl TestParams {
    /// The arguments as the harness takes them: a JSON object as text.
    ///
    /// # Errors
    ///
    /// [`sub_core::codes::INTERNAL`] if they cannot be serialised, which would
    /// be a bug here rather than something a caller can fix.
    pub fn args_json(&self) -> SubResult<String> {
        let arguments = self
            .arguments
            .clone()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        serde_json::to_string(&arguments).map_err(|err| {
            SubError::wrap(
                sub_core::codes::INTERNAL,
                "the test arguments could not be serialised",
                &err,
            )
        })
    }
}

/// How a host scaffolds a plugin crate.
pub type Scaffolder = Arc<dyn Fn(&NewParams) -> SubResult<Value> + Send + Sync>;

/// How a host runs the test harness over an installed plugin.
pub type Tester = Arc<dyn Fn(&TestParams) -> SubResult<Value> + Send + Sync>;

/// Puts `plugin.new` and `plugin.test` on a [`Dispatcher`], served by the
/// host's own scaffolder and tester.
///
/// They are queries: neither touches the open project, so nothing goes on the
/// undo stack. A plugin whose checks fail is *not* an error — the report comes
/// back with `ok: false`, which is what an agent reads to decide what to fix.
///
/// # Errors
///
/// `command.duplicate_method` when either name is already served.
pub fn register_methods(
    dispatcher: &mut Dispatcher,
    scaffolder: Scaffolder,
    tester: Tester,
) -> SubResult<()> {
    dispatcher.register::<NewParams, Value, _>(PLUGIN_NEW, NEW_DESCRIPTION, move |_, params| {
        let params: NewParams = typed(params)?;
        scaffolder(&params)
    })?;
    dispatcher.register::<TestParams, Value, _>(PLUGIN_TEST, TEST_DESCRIPTION, move |_, params| {
        let params: TestParams = typed(params)?;
        tester(&params)
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

/// The JSON Schema entries of the authoring methods, folded into the plugin
/// management document.
pub(crate) mod schema {
    use schemars::SchemaGenerator;
    use serde_json::Value;

    use super::{
        NEW_DESCRIPTION, NewParams, PLUGIN_NEW, PLUGIN_TEST, TEST_DESCRIPTION, TestParams,
    };

    /// One method's entry, built the way `registry::schema` builds one.
    ///
    /// Both results are free-form: a scaffold answers the files it wrote and a
    /// test answers the harness's report, and neither is a type a caller
    /// should be made to match field for field.
    fn method<P: schemars::JsonSchema>(
        generator: &mut SchemaGenerator,
        name: &str,
        description: &str,
    ) -> Value {
        serde_json::json!({
            "name": name,
            "kind": "query",
            "description": description,
            "params": generator.subschema_for::<P>().to_value(),
            "result": { "type": "object" },
        })
    }

    /// The two authoring methods, in name order.
    pub(crate) fn methods(generator: &mut SchemaGenerator) -> Vec<Value> {
        vec![
            method::<NewParams>(generator, PLUGIN_NEW, NEW_DESCRIPTION),
            method::<TestParams>(generator, PLUGIN_TEST, TEST_DESCRIPTION),
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use sub_command::Dispatcher;
    use sub_edit::Engine;
    use sub_model::Project;

    use super::{NewParams, PLUGIN_NEW, PLUGIN_TEST, TestParams, register_methods};
    use crate::manifest::World;

    /// A dispatcher whose two authoring methods echo their parameters.
    fn dispatcher() -> (Engine, Dispatcher) {
        let engine = Engine::spawn(Project::new("Authoring")).expect("an engine");
        let mut dispatcher = Dispatcher::new(engine.handle().clone());
        register_methods(
            &mut dispatcher,
            Arc::new(|params: &NewParams| Ok(json!({ "world": params.world.as_str() }))),
            Arc::new(|params: &TestParams| {
                Ok(json!({ "id": params.id.as_str(), "args": params.args_json()? }))
            }),
        )
        .expect("the authoring methods");
        (engine, dispatcher)
    }

    #[test]
    fn the_two_methods_are_served_and_carry_their_parameters_through() {
        let (engine, dispatcher) = dispatcher();
        let scaffolded = dispatcher
            .invoke(
                PLUGIN_NEW,
                Some(json!({ "name": "cut-silence", "world": "mcp-tools" })),
            )
            .expect("plugin.new is served");
        assert_eq!(scaffolded["world"], "mcp-tools");

        let tested = dispatcher
            .invoke(
                PLUGIN_TEST,
                Some(json!({
                    "id": "com.example.cut-silence",
                    "arguments": { "threshold_db": -40 },
                })),
            )
            .expect("plugin.test is served");
        assert_eq!(tested["id"], "com.example.cut-silence");
        assert_eq!(tested["args"], r#"{"threshold_db":-40}"#);
        engine.shutdown().expect("the engine stops");
    }

    #[test]
    fn parameters_that_do_not_match_are_invalid_params() {
        let (engine, dispatcher) = dispatcher();
        let error = dispatcher
            .invoke(PLUGIN_NEW, Some(json!({ "name": "cut-silence" })))
            .expect_err("world is required");
        assert_eq!(error.code.as_str(), "command.invalid_params");
        engine.shutdown().expect("the engine stops");
    }

    #[test]
    fn absent_test_arguments_are_an_empty_object() {
        let params = TestParams {
            id: crate::manifest::PluginId::parse("com.example.one").expect("an id"),
            fixture: None,
            arguments: None,
        };
        assert_eq!(params.args_json().expect("json"), "{}");
        assert_eq!(World::McpTools.as_str(), "mcp-tools");
        let _ = NewParams {
            name: "one".to_owned(),
            world: World::Command,
            parent: None,
            id: None,
            sdk_path: None,
            force: false,
        };
    }
}
