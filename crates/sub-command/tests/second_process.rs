//! The Command API from another process, which is the case that matters.
//!
//! `subordinate-mcp` and `subordinate-cli` are separate binaries: they find the
//! running editor through the endpoint's lock file and speak newline-delimited
//! JSON-RPC to it. This test plays both halves. The parent runs an engine and a
//! [`Server`]; the child is this same test binary re-executed with one ignored
//! test named, given nothing but the endpoint directory. Everything else — the
//! socket path or pipe name, the framing — it learns from the lock file.

use std::path::PathBuf;
use std::process::Command as Process;
use std::sync::Arc;

use serde_json::json;
use sub_command::Dispatcher;
use sub_command::endpoint::Endpoint;
use sub_command::transport::{Client, Server};
use sub_edit::Engine;
use sub_model::Project;

/// How the parent tells the child where to look.
const DIRECTORY_VAR: &str = "SUB_COMMAND_TEST_ENDPOINT_DIR";

/// The instance both halves agree on.
const INSTANCE: &str = "second-process";

/// What the child prints when it has run its command.
const DONE: &str = "second-process-ok revision=";

/// The bin the child creates, which the parent then finds in its own engine.
const BIN_NAME: &str = "From another process";

#[test]
fn a_second_process_connects_and_runs_a_command() {
    let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
    let directory = std::env::temp_dir().join(format!("sub-second-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let endpoint = Endpoint::in_directory(&directory, INSTANCE).unwrap();
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let server = Server::bind(endpoint, dispatcher).unwrap();

    let child = Process::new(std::env::current_exe().unwrap())
        .args([
            "client_in_a_second_process",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(DIRECTORY_VAR, &directory)
        .output()
        .expect("the test binary can be run again as a client");

    let stdout = String::from_utf8_lossy(&child.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&child.stderr).into_owned();
    assert!(
        child.status.success(),
        "the client process failed\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        stdout.contains(&format!("{DONE}1")),
        "the client did not report its command\nstdout:\n{stdout}",
    );

    // The edit the other process made is in this process's project.
    let project = engine.handle().snapshot();
    assert_eq!(project.root_bin.children.len(), 1);
    assert_eq!(project.root_bin.children[0].name, BIN_NAME);
    assert_eq!(engine.handle().revision(), 1);

    server.shutdown().unwrap();
    engine.shutdown().unwrap();
}

/// The client half, run as a child process by the test above.
///
/// It is ignored so that a plain `cargo test` runs it only through its parent,
/// and it does nothing at all when it is run without the parent's environment.
#[test]
#[ignore = "run as a child process by a_second_process_connects_and_runs_a_command"]
fn client_in_a_second_process() {
    let Ok(directory) = std::env::var(DIRECTORY_VAR) else {
        eprintln!("{DIRECTORY_VAR} is unset: this test only runs as a child process");
        return;
    };

    // All the client is given is the directory; the lock file supplies the rest.
    let endpoint = Endpoint::in_directory(PathBuf::from(directory), INSTANCE).unwrap();
    let lock = endpoint
        .read_lock()
        .expect("the lock file parses")
        .expect("a server published a lock file");
    assert_ne!(
        lock.pid,
        std::process::id(),
        "the server is another process"
    );

    let mut client = Client::connect(&endpoint).expect("the server answers");
    let applied = client
        .invoke("bin.create", Some(json!({ "name": BIN_NAME })))
        .expect("the command applies");
    let revision = applied["revision"].as_u64().expect("a revision came back");

    // A query over the same connection sees the command that just ran.
    let queried = client.invoke("project.revision", None).unwrap();
    assert_eq!(queried["revision"].as_u64(), Some(revision));

    // And an unknown method still fails as a SubError, not as a broken pipe.
    let error = client.invoke("nope.method", None).unwrap_err();
    assert_eq!(error.code.as_str(), "command.unknown_method");

    println!("{DONE}{revision}");
}
