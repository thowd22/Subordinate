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
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let server = Server::bind(endpoint, dispatcher).expect("a bound server");

    let backend = Backend::connect(&options(&directory)).expect("the bridge finds the editor");
    assert!(
        !backend.launched_server(),
        "an editor was already running, so nothing should have been launched",
    );
    let bridge = bridge(backend);

    // The whole Command API is offered, and `tools/list` says so.
    assert_eq!(bridge.tools().len(), 50);
    assert_eq!(bridge.tools().method("bin_create"), Some("bin.create"));
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

/// The `subordinate-cli` this test run built, when there is one.
///
/// Integration tests live in `target/<profile>/deps`, and the workspace's
/// binaries are one directory above that.
fn built_cli() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let candidate = executable.parent()?.parent()?.join(cli_name());
    candidate.is_file().then_some(candidate)
}
