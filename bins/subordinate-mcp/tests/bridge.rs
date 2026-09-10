//! The bridge against a real Command API.
//!
//! Two ways in, and both are exercised here: connecting to an editor that is
//! already listening, and starting `subordinate-cli serve` when none is. In
//! both cases a tool call has to reach the engine and come back as MCP content,
//! because that is the whole job of this binary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rmcp::ServerHandler as _;
use rmcp::model::CallToolRequestParams;
use serde_json::json;
use sub_command::Dispatcher;
use sub_command::endpoint::Endpoint;
use sub_command::transport::Server;
use sub_edit::Engine;
use sub_model::Project;
use sub_plugin::registry;
use subordinate_mcp::backend::{Backend, Options, cli_name};
use subordinate_mcp::bridge::Bridge;
use subordinate_mcp::tools::ToolSet;

/// The instance these tests serve, kept out of the way of a real editor.
const INSTANCE: &str = "mcp-bridge-test";

/// A directory of this test's own for the socket and the lock file.
fn workspace(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("sub-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");
    directory
}

/// How many methods the two committed schemas describe between them, which is
/// how many tools the bridge must offer.
fn method_count() -> usize {
    [
        subordinate_mcp::tools::COMMAND_API_SCHEMA,
        subordinate_mcp::tools::PLUGIN_API_SCHEMA,
    ]
    .into_iter()
    .map(|text| {
        serde_json::from_str::<serde_json::Value>(text).expect("a committed schema")["methods"]
            .as_array()
            .expect("a methods array")
            .len()
    })
    .sum()
}

/// The options that point the bridge at `directory`.
fn options(directory: &Path) -> Options {
    Options {
        instance: INSTANCE.to_owned(),
        directory: Some(directory.to_path_buf()),
        ..Options::default()
    }
}

/// A bridge over `backend`, with the committed tool set.
fn bridge(backend: Backend) -> Bridge {
    let tools = Arc::new(ToolSet::committed().expect("the committed tool set"));
    Bridge::new(tools, Arc::new(backend))
}

/// A call to one tool, with its arguments.
fn call(name: &str, arguments: &serde_json::Value) -> CallToolRequestParams {
    let mut request = CallToolRequestParams::new(name.to_owned());
    request.arguments = Some(
        arguments
            .as_object()
            .expect("arguments are an object")
            .clone(),
    );
    request
}

#[test]
fn tool_calls_reach_a_running_editor_and_come_back_as_content() {
    let directory = workspace("running");
    let engine = Engine::spawn(Project::new("Doc cut")).expect("an engine");
    let endpoint = Endpoint::in_directory(directory.clone(), INSTANCE).expect("an endpoint");
    let mut dispatcher = Dispatcher::new(engine.handle().clone());

    // The editor serves the plugin host's methods beside the engine's, which is
    // what makes the plugin tools reachable from here.
    let plugins = directory.join("plugins");
    std::fs::create_dir_all(plugins.join("com.example.demo")).expect("a plugin directory");
    std::fs::write(
        plugins.join("com.example.demo").join("plugin.toml"),
        "[plugin]\nid = \"com.example.demo\"\nname = \"Demo\"\nversion = \"0.1.0\"\n\
         api = \"0.1\"\nworlds = [\"command\"]\n",
    )
    .expect("a manifest");
    registry::register_methods(
        &mut dispatcher,
        Arc::new(registry::PluginRegistry::new(registry::PluginDirs::new(
            &plugins,
        ))),
    )
    .expect("the plugin methods");
    let server = Server::bind(endpoint, Arc::new(dispatcher)).expect("a bound server");

    let backend = Backend::connect(&options(&directory)).expect("the bridge finds the editor");
    assert!(
        !backend.launched_server(),
        "an editor was already running, so nothing should have been launched",
    );
    let bridge = bridge(backend);

    // The whole Command API is offered, and `tools/list` says so: the engine's
    // methods plus the plugin host's, which are exported as a second document.
    assert_eq!(bridge.tools().len(), method_count());
    assert_eq!(bridge.tools().method("bin_create"), Some("bin.create"));
    assert_eq!(bridge.tools().method("plugin_list"), Some("plugin.list"));
    let info = bridge.get_info();
    assert!(info.capabilities.tools.is_some());
    assert!(info.instructions.is_some());

    // A mutating tool is an undoable command in the engine.
    let created = bridge
        .call(call("bin_create", &json!({ "name": "Footage" })))
        .expect("the tool exists");
    assert_eq!(created.is_error, Some(false));
    let structured = created.structured_content.expect("structured content");
    assert_eq!(structured["revision"], 1);
    assert_eq!(
        engine.handle().snapshot().root_bin.children[0].name,
        "Footage",
    );

    let undone = bridge
        .call(call("edit_undo", &json!({})))
        .expect("undo is a tool");
    assert_eq!(undone.is_error, Some(false));
    assert!(engine.handle().snapshot().root_bin.children.is_empty());

    // The plugin management tools reach the registry the editor was given, so
    // an agent lists and switches plugins without leaving MCP.
    let listed = bridge
        .call(call("plugin_list", &json!({})))
        .expect("plugin.list is a tool");
    assert_eq!(listed.is_error, Some(false));
    let scan = listed.structured_content.expect("structured content");
    assert_eq!(scan["plugins"][0]["id"], "com.example.demo");
    assert_eq!(scan["plugins"][0]["enabled"], true);

    let disabled = bridge
        .call(call("plugin_disable", &json!({ "id": "com.example.demo" })))
        .expect("plugin.disable is a tool");
    assert_eq!(disabled.is_error, Some(false));
    let listed = bridge
        .call(call("plugin_list", &json!({})))
        .expect("plugin.list is a tool");
    let scan = listed.structured_content.expect("structured content");
    assert_eq!(scan["plugins"][0]["enabled"], false);

    let removed = bridge
        .call(call("plugin_remove", &json!({ "id": "com.example.demo" })))
        .expect("plugin.remove is a tool");
    assert_eq!(removed.is_error, Some(false));
    assert!(!plugins.join("com.example.demo").exists());

    // A failure is a tool error carrying the engine's stable code, not a
    // protocol error.
    let failed = bridge
        .call(call(
            "bin_rename",
            &json!({ "bin": "bin_absent", "name": "x" }),
        ))
        .expect("the tool exists");
    assert_eq!(failed.is_error, Some(true));
    let error = failed.structured_content.expect("structured content");
    let code = error["code"].as_str().expect("a code");
    assert!(code.contains('.'), "{code} is not a stable error code");

    // A tool this bridge does not serve is a protocol error.
    let unknown = bridge
        .call(call("bin.create", &json!({})))
        .expect_err("dotted names are not tool names");
    assert_eq!(unknown.code, rmcp::model::ErrorCode::INVALID_PARAMS);

    drop(bridge);
    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
}

#[test]
fn nothing_running_and_no_launching_is_reported_as_such() {
    let directory = workspace("absent");
    let error = Backend::connect(&Options {
        launch: false,
        ..options(&directory)
    })
    .expect_err("nothing is listening");
    assert_eq!(error.code.as_str(), "command.not_running");
}

#[test]
fn a_missing_headless_cli_is_reported_with_a_stable_code() {
    let directory = workspace("no-cli");
    let error = Backend::connect(&Options {
        cli: Some(directory.join("not-a-program")),
        ..options(&directory)
    })
    .expect_err("there is no such program");
    assert_eq!(error.code.as_str(), "mcp.launch_failed");
}

#[test]
fn with_no_editor_running_the_bridge_launches_a_headless_one() {
    let Some(cli) = built_cli() else {
        eprintln!("subordinate-cli is not built beside this test; skipping the launch path");
        return;
    };
    let directory = workspace("launched");
    let backend = Backend::connect(&Options {
        cli: Some(cli),
        ..options(&directory)
    })
    .expect("the bridge starts a headless server");
    assert!(backend.launched_server());

    let bridge = bridge(backend);
    let revision = bridge
        .call(call("project_revision", &json!({})))
        .expect("the tool exists");
    assert_eq!(revision.is_error, Some(false));
    assert_eq!(
        revision.structured_content.expect("structured content")["revision"],
        0,
    );

    // Dropping the bridge closes the child's stdin, which is how `serve` is
    // asked to stop; the lock file goes with it.
    drop(bridge);
    assert!(
        !Endpoint::in_directory(directory, INSTANCE)
            .expect("an endpoint")
            .lock_path()
            .exists(),
        "the launched server should have cleaned up after itself",
    );
}

/// A plugin whose manifest contributes one MCP tool.
fn install_plugin_with_a_tool(plugins: &Path, id: &str, tool: &str) {
    let directory = plugins.join(id);
    std::fs::create_dir_all(&directory).expect("a plugin directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "[plugin]\nid = \"{id}\"\nname = \"Demo\"\nversion = \"0.1.0\"\n\
             api = \"0.1\"\nworlds = [\"command\", \"mcp-tools\"]\n\n\
             [mcp.tools.{tool}]\ndescription = \"Cut the quiet bits\"\n\
             schema = \"{tool}.json\"\n"
        ),
    )
    .expect("a manifest");
    std::fs::write(
        directory.join(format!("{tool}.json")),
        r#"{"type":"object","properties":{"threshold_db":{"type":"number"}}}"#,
    )
    .expect("a tool schema");
}

#[test]
fn a_plugins_tools_are_offered_under_its_id_and_route_back_to_it() {
    let directory = workspace("plugin-tools");
    let engine = Engine::spawn(Project::new("Doc cut")).expect("an engine");
    let endpoint = Endpoint::in_directory(directory.clone(), INSTANCE).expect("an endpoint");
    let mut dispatcher = Dispatcher::new(engine.handle().clone());

    let plugins = directory.join("plugins");
    install_plugin_with_a_tool(&plugins, "com.example.demo", "cut_silence");
    let registry = Arc::new(registry::PluginRegistry::new(registry::PluginDirs::new(
        &plugins,
    )));
    registry::register_methods(&mut dispatcher, Arc::clone(&registry)).expect("the methods");
    let server = Server::bind(endpoint, Arc::new(dispatcher)).expect("a bound server");

    let bridge = bridge(Backend::connect(&options(&directory)).expect("the editor"));

    // The plugin's tool is offered beside the compiled-in ones, named for the
    // plugin that contributes it and carrying the manifest's description and
    // schema.
    let published = bridge.published_tools();
    assert_eq!(published.len(), method_count() + 1);
    let tool = published
        .iter()
        .find(|tool| tool.name == "com_example_demo_cut_silence")
        .expect("the plugin's tool is published under its id");
    assert_eq!(tool.title.as_deref(), Some("com.example.demo.cut_silence"));
    assert_eq!(tool.description.as_deref(), Some("Cut the quiet bits"));
    assert_eq!(tool.input_schema["type"], "object");

    let plugin_tools = bridge.plugin_tools();
    let route = plugin_tools
        .route("com_example_demo_cut_silence")
        .expect("the published name routes back to the plugin");
    assert_eq!(route.plugin, "com.example.demo");
    assert_eq!(route.tool, "cut_silence");

    // Calling it is a call this bridge routes rather than refuses: it goes out
    // as plugin.call_tool, which this editor does not serve — it registered the
    // registry's methods and no plugin host — so the answer is the Command
    // API's own stable code and not a protocol error. `tests/plugin_tools.rs`
    // is the same call against an editor that does serve it.
    let forwarded = bridge
        .call(call(
            "com_example_demo_cut_silence",
            &json!({ "threshold_db": -40 }),
        ))
        .expect("a plugin tool is a tool this bridge serves");
    assert_eq!(forwarded.is_error, Some(true));
    let error = forwarded.structured_content.expect("structured content");
    assert_eq!(error["code"], "command.unknown_method");

    // A plugin switched off contributes nothing.
    let id = sub_plugin::PluginId::parse("com.example.demo").expect("a valid id");
    registry.set_enabled(&id, false).expect("disabled");
    assert_eq!(bridge.published_tools().len(), method_count());
    assert!(bridge.plugin_tools().is_empty());

    drop(bridge);
    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
}

/// The `subordinate-cli` this test run built, when there is one.
///
/// Integration tests live in `target/<profile>/deps`, and the workspace's
/// binaries are one directory above that.
fn built_cli() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let candidate = executable.parent()?.parent()?.join(cli_name());
    candidate.is_file().then_some(candidate)
}
