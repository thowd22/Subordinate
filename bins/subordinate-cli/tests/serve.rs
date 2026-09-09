//! `subordinate-cli serve` exposes the Command API socket without a GUI.
//!
//! This is what `subordinate-mcp` launches when no editor window is running,
//! so the test plays that part: it starts the server as its own process, finds
//! the endpoint through the readiness line and the lock file exactly as
//! another program would, runs a command over the socket and then asks the
//! server to stop by closing its stdin.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};

use serde_json::{Value, json};
use sub_command::endpoint::Endpoint;
use sub_command::transport::Client;

/// The instance this test serves, kept out of the way of a real editor.
const INSTANCE: &str = "cli-serve-test";

/// A directory of this test's own for the socket and the lock file.
fn workspace(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("sub-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");
    directory
}

/// Runs the CLI to completion and parses the JSON it printed.
fn cli_json(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_subordinate-cli"))
        .args(args)
        .env("SUBORDINATE_LOG", "warn")
        .env_remove("RUST_LOG")
        .output()
        .expect("subordinate-cli runs");
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the CLI printed JSON")
}

/// Starts `serve` and reads its readiness line.
fn start(args: &[&str]) -> (Child, BufReader<ChildStdout>, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_subordinate-cli"))
        .args(args)
        .env("SUBORDINATE_LOG", "warn")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("subordinate-cli serve starts");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut line = String::new();
    stdout
        .read_line(&mut line)
        .expect("the server announces itself");
    let ready: Value = serde_json::from_str(&line)
        .unwrap_or_else(|err| panic!("the readiness line is not JSON ({err}): {line:?}"));
    (child, stdout, ready)
}

#[test]
fn serve_exposes_the_command_api_socket_without_a_gui() {
    let directory = workspace("serve");
    let project = directory.join("served.sub");
    let created = cli_json(&["new", &project.display().to_string(), "--compact"]);
    let sequence = created["sequences"][0]["id"]
        .as_str()
        .expect("the starter sequence id")
        .to_owned();

    let (mut child, mut stdout, ready) = start(&[
        "serve",
        "--directory",
        &directory.display().to_string(),
        "--instance",
        INSTANCE,
        "--project",
        &project.display().to_string(),
        "--compact",
    ]);

    assert_eq!(ready["instance"], INSTANCE);
    assert_eq!(ready["project"]["name"], "served");
    let lock_file = PathBuf::from(ready["lock_file"].as_str().expect("a lock file path"));
    assert!(lock_file.exists(), "the lock file was not written");
    let recorded: Value =
        serde_json::from_str(&std::fs::read_to_string(&lock_file).expect("the lock file reads"))
            .expect("the lock file is JSON");
    assert_eq!(recorded["address"], ready["address"]);

    // A second process finds the endpoint the way the MCP bridge does: from
    // the directory and the instance name alone.
    let endpoint = Endpoint::in_directory(&directory, INSTANCE).expect("the endpoint");
    let mut client = Client::connect(&endpoint).expect("a client connects to the served endpoint");

    let project_state = client
        .invoke("project.get", None)
        .expect("project.get answers");
    // `project.get` answers with the project file, so the model sits one
    // level in under its `project` member.
    assert_eq!(project_state["project"]["project"]["name"], "served");
    assert_eq!(project_state["revision"], 0);

    let methods = client
        .invoke("system.list_methods", None)
        .expect("system.list_methods answers");
    let names: Vec<&str> = methods["methods"]
        .as_array()
        .expect("a method list")
        .iter()
        .map(|method| method["name"].as_str().expect("a method name"))
        .collect();
    assert!(names.contains(&"track.add"), "methods: {names:?}");

    // The full command set is served, not just queries: this mutation goes
    // through the same undoable Command path the GUI uses.
    let applied = client
        .invoke(
            "track.add",
            Some(json!({ "sequence": sequence, "name": "V2", "kind": "video" })),
        )
        .expect("track.add applies");
    assert_eq!(applied["revision"], 1);
    let history = client.invoke("history.get", None).expect("history.get");
    assert_eq!(history["can_undo"], true);

    drop(client);

    // Closing stdin is how the parent process asks the server to stop.
    drop(child.stdin.take().expect("piped stdin"));
    let mut line = String::new();
    stdout.read_line(&mut line).expect("the server signs off");
    let report: Value = serde_json::from_str(&line).expect("the final report is JSON");
    assert_eq!(report["stopped"], true);
    assert_eq!(report["address"], ready["address"]);

    let status = child.wait().expect("the server exits");
    assert!(status.success(), "the server exited with {status}");
    assert!(
        !lock_file.exists(),
        "the lock file outlived the server it advertised"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn serve_without_a_project_still_binds_an_endpoint() {
    let directory = workspace("serve-empty");
    let (mut child, mut stdout, ready) = start(&[
        "serve",
        "--directory",
        &directory.display().to_string(),
        "--instance",
        "cli-serve-empty",
        "--compact",
    ]);

    assert_eq!(ready["project"]["name"], "Untitled");
    assert_eq!(ready["project"]["path"], Value::Null);
    assert_eq!(ready["pid"], u64::from(child.id()));

    let endpoint = Endpoint::in_directory(&directory, "cli-serve-empty").expect("the endpoint");
    let mut client = Client::connect(&endpoint).expect("a client connects");
    let revision = client
        .invoke("project.revision", None)
        .expect("project.revision answers");
    assert_eq!(revision["revision"], 0);
    drop(client);

    drop(child.stdin.take().expect("piped stdin"));
    let mut line = String::new();
    stdout.read_line(&mut line).expect("the server signs off");
    assert!(child.wait().expect("the server exits").success());

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn serve_refuses_a_project_file_it_cannot_read() {
    let directory = workspace("serve-missing");
    let output = Command::new(env!("CARGO_BIN_EXE_subordinate-cli"))
        .args([
            "serve",
            "--directory",
            &directory.display().to_string(),
            "--instance",
            "cli-serve-missing",
            "--project",
            &directory.join("absent.sub").display().to_string(),
            "--compact",
        ])
        .env("SUBORDINATE_LOG", "warn")
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .output()
        .expect("subordinate-cli runs");
    assert!(!output.status.success(), "serve accepted a missing project");
    let error: Value = serde_json::from_slice(&output.stderr).expect("a JSON SubError on stderr");
    assert_eq!(error["code"], "core.io");
    assert!(
        output.stdout.is_empty(),
        "nothing was served, so nothing is announced"
    );

    let _ = std::fs::remove_dir_all(&directory);
}
