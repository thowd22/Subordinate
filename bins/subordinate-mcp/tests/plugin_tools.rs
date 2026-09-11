//! Installing a plugin over MCP and calling the tool it contributes.
//!
//! This is the loop TASK-96 closes, end to end and with a real component: the
//! agent calls `plugin_install`, the editor loads the plugin and publishes its
//! tool under the plugin's id, `tools/list` offers it beside the compiled-in
//! ones, and calling it runs WASM that edits the project through the Command
//! API. Nothing here is stubbed — the editor side is the same registry, the
//! same [`DevHost`] and the same harness-backed tool caller `subordinate-cli
//! serve` installs.
//!
//! It needs the `wasm32-wasip2` guest that `sub-plugin`'s build script
//! compiles. On a machine without that target the guest is not built and the
//! test says so and passes, the way the plugin host's own runtime tests do.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult};
use serde_json::{Value, json};
use sub_command::Dispatcher;
use sub_command::endpoint::Endpoint;
use sub_command::transport::Server;
use sub_edit::Engine;
use sub_model::Project;
use sub_plugin::dev::{self, DevHost};
use sub_plugin::harness::Harness;
use sub_plugin::registry::{self, PluginDirs, PluginRegistry};
use sub_plugin::runtime::PluginRuntime;
use subordinate_mcp::backend::{Backend, Options};
use subordinate_mcp::bridge::Bridge;
use subordinate_mcp::tools::ToolSet;

/// The instance this test serves, kept out of the way of a real editor.
///
/// Short on purpose: with the directory below it this becomes a Unix-domain
/// socket path, and macOS temporary directories leave barely a hundred bytes
/// for the whole thing.
const INSTANCE: &str = "mcp-plugin";

/// The plugin the guest is installed as, and the tool it contributes.
const PLUGIN_ID: &str = "com.example.toolbox";
/// The tool's plugin-local name.
const TOOL: &str = "create_bin";
/// The name the bridge publishes it under: the plugin id with its dots turned
/// into underscores, then the tool's own name.
const PUBLISHED: &str = "com_example_toolbox_create_bin";

/// The tool's argument schema, byte for byte the one the guest exports. The
/// host refuses to publish a tool whose manifest and component disagree.
const SCHEMA: &str =
    r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"#;

/// A directory of this test's own for the socket, the lock file and the
/// plugins. Kept short: the socket path built inside it has to fit in a
/// `sockaddr_un`, which macOS caps at 104 bytes.
fn workspace(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("sub-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");
    directory
}

/// A call to one tool, with its arguments.
fn call(name: &str, arguments: &Value) -> CallToolRequestParams {
    let mut request = CallToolRequestParams::new(name.to_owned());
    request.arguments = Some(arguments.as_object().expect("an object").clone());
    request
}

/// One tool call, run to completion.
///
/// A destructive tool answers `input_required` before it runs anything (see
/// `tests/confirmations.rs`); nothing called here is one, so every call
/// finishes in a single round trip.
fn run(bridge: &Bridge, request: CallToolRequestParams) -> Result<CallToolResult, rmcp::ErrorData> {
    bridge.call(request).map(|response| match response {
        CallToolResponse::Complete(result) => result,
        other => panic!("the call did not complete: {other:?}"),
    })
}

/// Lays out a plugin source directory around the built guest component: the
/// manifest that declares the tool, its schema file, and the `.wasm` itself.
fn plugin_source(root: &Path, component: &Path) -> PathBuf {
    let source = root.join("src-toolbox");
    std::fs::create_dir_all(&source).expect("a source directory");
    std::fs::write(
        source.join("plugin.toml"),
        format!(
            "[plugin]\nid = \"{PLUGIN_ID}\"\nname = \"Toolbox\"\nversion = \"0.1.0\"\n\
             api = \"0.1\"\nworlds = [\"mcp-tools\"]\n\n\
             [mcp.tools.{TOOL}]\ndescription = \"Create a bin in the open project.\"\n\
             schema = \"{TOOL}.json\"\n"
        ),
    )
    .expect("a manifest");
    std::fs::write(source.join(format!("{TOOL}.json")), SCHEMA).expect("a tool schema");
    std::fs::copy(component, source.join("plugin.wasm")).expect("the built component");
    source
}

/// The editor side: the engine, the plugin host with a real tool caller, and
/// the socket the bridge connects to.
fn editor(directory: &Path) -> (Engine, Server) {
    let engine = Engine::spawn(Project::new("Doc cut")).expect("an engine");
    let endpoint = Endpoint::in_directory(directory.to_path_buf(), INSTANCE).expect("an endpoint");
    let mut dispatcher = Dispatcher::new(engine.handle().clone());

    let registry = Arc::new(PluginRegistry::new(PluginDirs::new(
        directory.join("plugins"),
    )));
    registry::register_methods(&mut dispatcher, Arc::clone(&registry)).expect("registry methods");

    let runtime = PluginRuntime::new().expect("a wasm engine");
    // The plugin's own view of the Command API: the engine's methods, and none
    // of the plugin management ones, exactly as `subordinate-cli serve` builds
    // it.
    let plugin_facing = Arc::new(Dispatcher::new(engine.handle().clone()));
    let harness = Harness::new(engine.handle().clone(), plugin_facing).expect("a harness");
    let host = DevHost::with_loader(registry, dev::manifest_tools_loader(runtime))
        .with_tool_caller(dev::harness_tool_caller(Arc::new(harness)));
    dev::register_methods(&mut dispatcher, Arc::new(Mutex::new(host))).expect("dev methods");

    let server = Server::bind(endpoint, Arc::new(dispatcher)).expect("a bound server");
    (engine, server)
}

#[test]
fn a_plugin_installed_over_mcp_contributes_a_tool_an_agent_can_call() {
    let Some(component) = sub_plugin::guests::TOOLBOX else {
        eprintln!("the wasm32-wasip2 target is not installed; skipping the plugin tool test");
        return;
    };
    let directory = workspace("pcall");
    let source = plugin_source(&directory, Path::new(component));
    let (engine, server) = editor(&directory);

    let backend = Backend::connect(&Options {
        instance: INSTANCE.to_owned(),
        directory: Some(directory.clone()),
        ..Options::default()
    })
    .expect("the bridge finds the editor");
    let bridge = Bridge::new(
        Arc::new(ToolSet::committed().expect("the committed tool set")),
        Arc::new(backend),
    );

    // Before anything is installed the bridge offers only the compiled-in
    // tools, and the plugin's name is not one of them.
    let before = bridge.published_tools().len();
    assert!(bridge.plugin_tools().is_empty());

    // The agent installs its plugin: one tool call, no shell.
    let installed = run(
        &bridge,
        call(
            "plugin_install",
            &json!({ "path": source.display().to_string(), "dev": true }),
        ),
    )
    .expect("plugin.install is a tool");
    let report = installed
        .structured_content
        .clone()
        .expect("structured content");
    assert_eq!(installed.is_error, Some(false), "{report}");
    assert_eq!(report["id"], PLUGIN_ID);

    // The tool it contributes is now offered, under the plugin's id, with the
    // manifest's description and schema.
    let published = bridge.published_tools();
    assert_eq!(published.len(), before + 1);
    let tool = published
        .iter()
        .find(|tool| tool.name == PUBLISHED)
        .expect("the plugin's tool is published under its id");
    assert_eq!(
        tool.title.as_deref(),
        Some("com.example.toolbox.create_bin"),
    );
    assert_eq!(
        tool.description.as_deref(),
        Some("Create a bin in the open project."),
    );
    assert_eq!(tool.input_schema["properties"]["name"]["type"], "string");

    // Calling it runs the component. The bin it creates is an ordinary
    // undoable command on the editor's own stack, which is the whole claim.
    let answered = run(&bridge, call(PUBLISHED, &json!({ "name": "Footage" })))
        .expect("a plugin tool is a tool this bridge serves");
    let structured = answered
        .structured_content
        .clone()
        .expect("structured content");
    assert_eq!(answered.is_error, Some(false), "{structured}");
    assert_eq!(structured["plugin"], PLUGIN_ID);
    assert_eq!(structured["tool"], TOOL);
    assert_eq!(structured["answer"]["created"]["revision"], 1);
    assert_eq!(
        engine.handle().snapshot().root_bin.children[0].name,
        "Footage",
    );

    let undone = run(&bridge, call("edit_undo", &json!({}))).expect("undo is a tool");
    assert_eq!(undone.is_error, Some(false));
    assert!(
        engine.handle().snapshot().root_bin.children.is_empty(),
        "a plugin's edit must be undoable like any other",
    );

    // Arguments the tool's schema rejects never reach the plugin, and come
    // back as the host's own stable code.
    let refused = run(&bridge, call(PUBLISHED, &json!({ "name": 7 }))).expect("the tool exists");
    assert_eq!(refused.is_error, Some(true));
    let error = refused.structured_content.expect("structured content");
    assert_eq!(error["code"], "plugin.invalid_tool_arguments");

    // A plugin switched off contributes nothing, and the listing says so —
    // which is what makes tools/list_changed worth sending.
    let disabled = run(&bridge, call("plugin_disable", &json!({ "id": PLUGIN_ID })))
        .expect("plugin.disable is a tool");
    assert_eq!(disabled.is_error, Some(false));
    // `published_tools` is what refreshes the cache, so it comes first; what it
    // leaves behind is the listing a client would be told about.
    assert_eq!(bridge.published_tools().len(), before);
    assert!(bridge.plugin_tools().is_empty());

    drop(bridge);
    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
}
