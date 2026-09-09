//! The binary itself, spoken to the way an MCP client speaks to it.
//!
//! This is the acceptance test for the bridge: start `subordinate-mcp` as its
//! own process, run the MCP initialize handshake over its stdin and stdout,
//! list the tools and call one, and see the change land in the engine on the
//! other side of the socket. Nothing here uses the library, so the protocol,
//! the framing and the process lifetime are all under test.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;

use serde_json::{Value, json};
use sub_command::Dispatcher;
use sub_command::endpoint::Endpoint;
use sub_command::transport::Server;
use sub_edit::Engine;
use sub_model::Project;

/// The instance this test serves, kept out of the way of a real editor.
const INSTANCE: &str = "mcp-stdio-test";

/// An MCP client the length of one test.
struct Client {
    /// The bridge process.
    child: Child,
    /// Its standard input, which carries requests.
    stdin: ChildStdin,
    /// Its standard output, which carries responses.
    stdout: BufReader<ChildStdout>,
    /// The next request id.
    next_id: u64,
}

impl Client {
    /// Starts the bridge, pointed at `directory`.
    fn start(directory: &PathBuf) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_subordinate-mcp"))
            .env("SUBORDINATE_ENDPOINT_DIR", directory)
            .env("SUBORDINATE_INSTANCE", INSTANCE)
            .env("SUBORDINATE_MCP_NO_LAUNCH", "1")
            .env("SUBORDINATE_LOG", "warn")
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("subordinate-mcp starts");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    /// Sends one JSON-RPC message.
    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").expect("the bridge takes a message");
        self.stdin.flush().expect("the message goes out");
    }

    /// Reads one JSON-RPC message.
    fn receive(&mut self) -> Value {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).expect("a reply");
        assert!(read > 0, "the bridge closed its output");
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("{line:?} is not JSON: {error}"))
    }

    /// Sends a request and returns its `result`, failing on an error reply.
    fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        let reply = self.receive();
        assert_eq!(reply["id"], id, "{method} answered another request");
        assert!(reply.get("error").is_none(), "{method} failed: {reply}");
        reply["result"].clone()
    }

    /// Runs the MCP handshake and returns the server's information.
    fn initialize(&mut self) -> Value {
        let result = self.request(
            "initialize",
            &json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "subordinate-stdio-test", "version": "0.1.0" },
            }),
        );
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        result
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Closing stdin is how an MCP client ends a stdio session.
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_binary_speaks_mcp_over_stdio_and_drives_the_command_api() {
    let directory = std::env::temp_dir().join(format!("sub-mcp-stdio-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");

    let engine = Engine::spawn(Project::new("Doc cut")).expect("an engine");
    let endpoint = Endpoint::in_directory(directory.clone(), INSTANCE).expect("an endpoint");
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let server = Server::bind(endpoint, dispatcher).expect("a bound server");

    let mut client = Client::start(&directory);
    let initialized = client.initialize();
    assert_eq!(initialized["serverInfo"]["name"], "subordinate-mcp");
    assert!(initialized["capabilities"]["tools"].is_object());

    // Every Command API method is offered, with the schema's own description.
    let listed = client.request("tools/list", &json!({}));
    let tools = listed["tools"].as_array().expect("a tool list");
    assert_eq!(tools.len(), 45);
    let create = tools
        .iter()
        .find(|tool| tool["name"] == "bin_create")
        .expect("bin.create is a tool");
    assert_eq!(create["title"], "bin.create");
    assert_eq!(
        create["description"],
        "Create a new bin inside a parent bin."
    );
    assert_eq!(create["inputSchema"]["type"], "object");

    // Calling one reaches the engine over the socket.
    let called = client.request(
        "tools/call",
        &json!({ "name": "bin_create", "arguments": { "name": "Footage" } }),
    );
    assert_ne!(called["isError"], json!(true), "{called}");
    assert_eq!(called["structuredContent"]["revision"], 1);
    assert_eq!(
        engine.handle().snapshot().root_bin.children[0].name,
        "Footage",
    );

    // A rejected call is a tool error carrying the engine's stable code.
    let failed = client.request(
        "tools/call",
        &json!({ "name": "bin_rename", "arguments": { "bin": "bin_absent", "name": "x" } }),
    );
    assert_eq!(failed["isError"], json!(true));
    let code = failed["structuredContent"]["code"]
        .as_str()
        .expect("a stable error code");
    assert!(code.contains('.'), "{code} is not a stable error code");

    drop(client);
    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
    let _ = std::fs::remove_dir_all(&directory);
}
