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
use sub_model::{Project, Sequence, SequenceSettings};

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
    /// Notifications that arrived while a reply was being waited for.
    notifications: Vec<Value>,
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
            notifications: Vec::new(),
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

    /// Sends a request and reads messages until its reply arrives.
    ///
    /// Notifications may arrive at any time once something is subscribed, so
    /// anything that is not this reply is set aside for [`Client::notified`]
    /// rather than mistaken for one.
    fn reply(&mut self, method: &str, params: &Value) -> Value {
        let id = self.open(method, params);
        loop {
            let message = self.receive();
            if message["id"] == json!(id) {
                return message;
            }
            assert!(
                message.get("id").is_none(),
                "{method} answered another request: {message}",
            );
            self.notifications.push(message);
        }
    }

    /// Sends a request and returns its `result`, failing on an error reply.
    fn request(&mut self, method: &str, params: &Value) -> Value {
        let reply = self.reply(method, params);
        assert!(reply.get("error").is_none(), "{method} failed: {reply}");
        reply["result"].clone()
    }

    /// Sends a request that is expected to fail, and returns its `error`.
    fn failure(&mut self, method: &str, params: &Value) -> Value {
        let reply = self.reply(method, params);
        assert!(reply.get("result").is_none(), "{method} succeeded: {reply}");
        reply["error"].clone()
    }

    /// Waits for a notification of `method`, reading the stream until it comes.
    fn notified(&mut self, method: &str) -> Value {
        if let Some(index) = self
            .notifications
            .iter()
            .position(|message| message["method"] == method)
        {
            return self.notifications.remove(index);
        }
        loop {
            let message = self.receive();
            if message["method"] == method {
                return message;
            }
            assert!(
                message.get("id").is_none() || message.get("method").is_some(),
                "waiting for {method}, but a reply arrived: {message}",
            );
        }
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

    /// Sends a request without waiting for its reply, and returns its id.
    fn open(&mut self, method: &str, params: &Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        id
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

#[test]
fn the_binary_serves_the_project_as_resources_and_says_when_they_change() {
    let directory = std::env::temp_dir().join(format!("sub-mcp-stdio-res-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");

    let mut project = Project::new("Doc cut");
    project
        .sequences
        .push(Sequence::new("Timeline 1", SequenceSettings::default()));
    let sequence = project.sequences[0].id.to_string();
    let sequence_uri = format!("sequence://{sequence}");

    let engine = Engine::spawn(project).expect("an engine");
    let endpoint = Endpoint::in_directory(directory.clone(), INSTANCE).expect("an endpoint");
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let server = Server::bind(endpoint, dispatcher).expect("a bound server");

    let mut client = Client::start(&directory);
    let initialized = client.initialize();
    let resources = &initialized["capabilities"]["resources"];
    assert_eq!(resources["subscribe"], json!(true));
    assert_eq!(resources["listChanged"], json!(true));

    // Every list result carries the 2026-07-28 caching hints.
    let tools = client.request("tools/list", &json!({}));
    assert!(
        tools["ttlMs"].as_u64().is_some_and(|ttl| ttl > 0),
        "{tools}"
    );
    assert_eq!(tools["cacheScope"], "public");

    let listed = client.request("resources/list", &json!({}));
    assert!(
        listed["ttlMs"].as_u64().is_some_and(|ttl| ttl > 0),
        "{listed}",
    );
    assert_eq!(listed["cacheScope"], "private");
    let uris: Vec<&str> = listed["resources"]
        .as_array()
        .expect("a resource list")
        .iter()
        .map(|resource| resource["uri"].as_str().expect("a uri"))
        .collect();
    assert_eq!(uris, ["project://current", sequence_uri.as_str()]);

    let templates = client.request("resources/templates/list", &json!({}));
    let patterns: Vec<&str> = templates["resourceTemplates"]
        .as_array()
        .expect("templates")
        .iter()
        .map(|template| template["uriTemplate"].as_str().expect("a template"))
        .collect();
    assert_eq!(patterns, ["sequence://{id}", "media://{id}"]);
    assert!(templates["ttlMs"].as_u64().is_some());

    // A read is the project's own JSON, and it caches too.
    let read = client.request("resources/read", &json!({ "uri": "project://current" }));
    assert!(read["ttlMs"].as_u64().is_some_and(|ttl| ttl > 0), "{read}");
    assert_eq!(read["cacheScope"], "private");
    let contents = &read["contents"][0];
    assert_eq!(contents["uri"], "project://current");
    assert_eq!(contents["mimeType"], "application/json");
    let whole: Value =
        serde_json::from_str(contents["text"].as_str().expect("text")).expect("JSON contents");
    assert_eq!(whole["revision"], 0);
    assert_eq!(whole["project"]["name"], "Doc cut");
    assert_eq!(whole["project"]["sequences"][0]["id"], sequence);

    let read = client.request("resources/read", &json!({ "uri": sequence_uri }));
    let contents = &read["contents"][0];
    let timeline: Value =
        serde_json::from_str(contents["text"].as_str().expect("text")).expect("JSON contents");
    assert_eq!(timeline["sequence"]["name"], "Timeline 1");

    // A URI this bridge does not serve is a protocol error, not empty content.
    let error = client.failure("resources/read", &json!({ "uri": "clip://c1" }));
    assert_eq!(error["data"]["code"], "mcp.unknown_resource");

    // Subscribing means being told when an edit makes the resource stale.
    client.request("resources/subscribe", &json!({ "uri": sequence_uri }));
    let called = client.request(
        "tools/call",
        &json!({
            "name": "track_add",
            "arguments": { "sequence": sequence, "name": "V1", "kind": "video" },
        }),
    );
    assert_ne!(called["isError"], json!(true), "{called}");
    let update = client.notified("notifications/resources/updated");
    assert_eq!(update["params"]["uri"], sequence_uri);

    // And a subscription can be dropped again.
    client.request("resources/unsubscribe", &json!({ "uri": sequence_uri }));

    drop(client);
    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
    let _ = std::fs::remove_dir_all(&directory);
}
