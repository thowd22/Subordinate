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
use subordinate_mcp::tools::ToolSet;

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
        let executable = std::env::var_os("SUBORDINATE_MCP")
            .map(PathBuf::from)
            .or_else(|| {
                let sibling = std::env::current_exe()
                    .ok()?
                    .parent()?
                    .join(format!("subordinate-mcp{}", std::env::consts::EXE_SUFFIX));
                sibling.is_file().then_some(sibling)
            })
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_subordinate-mcp")));
        let mut child = Command::new(executable)
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

    // Every Command API method is offered, with the schema's own description,
    // and so are the plugin host's, exported as their own document.
    let listed = client.request("tools/list", &json!({}));
    let tools = listed["tools"].as_array().expect("a tool list");
    assert_eq!(
        tools.len(),
        ToolSet::committed().expect("the committed tools").len()
    );
    assert!(
        tools.iter().any(|tool| tool["name"] == "plugin_list"),
        "plugin.list is not offered",
    );
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

/// The plugin the reload test installs.
const PLUGIN_ID: &str = "com.example.toolbox";

/// The manifest of that plugin, with `description` for its one tool.
fn toolbox_manifest(description: &str) -> String {
    format!(
        "[plugin]\nid = \"{PLUGIN_ID}\"\nname = \"Toolbox\"\nversion = \"0.1.0\"\n\
         api = \"0.1\"\nworlds = [\"mcp-tools\"]\n\n\
         [mcp.tools.create_bin]\ndescription = \"{description}\"\n\
         schema = \"create_bin.json\"\n"
    )
}

/// Installing a plugin and reloading it both move the tool list under a client
/// that is already holding one, so both must be announced (TASK-96).
#[test]
fn installing_and_reloading_a_plugin_tells_the_client_its_tools_changed() {
    let Some(component) = sub_plugin::guests::TOOLBOX else {
        eprintln!("the wasm32-wasip2 target is not installed; skipping the reload notice test");
        return;
    };
    let directory = std::env::temp_dir().join(format!("sub-mcp-reload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");

    // A plugin source tree the agent will point `plugin_install` at.
    let source = directory.join("src-toolbox");
    std::fs::create_dir_all(&source).expect("a source directory");
    std::fs::write(
        source.join("plugin.toml"),
        toolbox_manifest("Create a bin."),
    )
    .expect("a manifest");
    std::fs::write(
        source.join("create_bin.json"),
        r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"#,
    )
    .expect("a tool schema");
    std::fs::copy(component, source.join("plugin.wasm")).expect("the built component");

    // The editor side: the engine's methods, the registry's and the plugin
    // host's, which is what `subordinate-cli serve` puts on one dispatcher.
    let engine = Engine::spawn(Project::new("Doc cut")).expect("an engine");
    let endpoint = Endpoint::in_directory(directory.clone(), INSTANCE).expect("an endpoint");
    let mut dispatcher = Dispatcher::new(engine.handle().clone());
    let registry = Arc::new(sub_plugin::registry::PluginRegistry::new(
        sub_plugin::registry::PluginDirs::new(directory.join("plugins")),
    ));
    sub_plugin::registry::register_methods(&mut dispatcher, Arc::clone(&registry))
        .expect("registry methods");
    let runtime = sub_plugin::runtime::PluginRuntime::new().expect("a wasm engine");
    let host = sub_plugin::dev::DevHost::with_loader(
        registry,
        sub_plugin::dev::manifest_tools_loader(runtime),
    );
    sub_plugin::dev::register_methods(&mut dispatcher, Arc::new(std::sync::Mutex::new(host)))
        .expect("dev methods");
    let server = Server::bind(endpoint, Arc::new(dispatcher)).expect("a bound server");

    let mut client = Client::start(&directory);
    let initialized = client.initialize();
    assert_eq!(
        initialized["capabilities"]["tools"]["listChanged"],
        json!(true),
        "a client must be told the tool list can change",
    );

    // The client lists once, so it now holds a listing with no plugin tools in
    // it. That listing is what the notifications below are about.
    let before = client.request("tools/list", &json!({}));
    let count = before["tools"].as_array().expect("a tool list").len();

    // Installing the plugin adds its tool, so the held listing is stale.
    let installed = client.request(
        "tools/call",
        &json!({
            "name": "plugin_install",
            "arguments": { "path": source.display().to_string(), "dev": true },
        }),
    );
    assert_ne!(installed["isError"], json!(true), "{installed}");
    client.notified("notifications/tools/list_changed");

    let after = client.request("tools/list", &json!({}));
    let tools = after["tools"].as_array().expect("a tool list");
    assert_eq!(tools.len(), count + 1);
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "com_example_toolbox_create_bin")
        .expect("the plugin's tool is offered under its id");
    assert_eq!(tool["description"], "Create a bin.");
    // A listing that carries a plugin's tools is this user's alone.
    assert_eq!(after["cacheScope"], "private");

    // The developer loop: edit the plugin, reload it, and the client is told
    // that what it is holding has moved on.
    std::fs::write(
        source.join("plugin.toml"),
        toolbox_manifest("Create a bin, now with a better description."),
    )
    .expect("the edited manifest");
    let reloaded = client.request(
        "tools/call",
        &json!({ "name": "plugin_reload", "arguments": { "id": PLUGIN_ID } }),
    );
    assert_ne!(reloaded["isError"], json!(true), "{reloaded}");
    client.notified("notifications/tools/list_changed");

    let after = client.request("tools/list", &json!({}));
    let tool = after["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .find(|tool| tool["name"] == "com_example_toolbox_create_bin")
        .expect("the plugin's tool is still offered");
    assert_eq!(
        tool["description"],
        "Create a bin, now with a better description.",
    );

    drop(client);
    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
    let _ = std::fs::remove_dir_all(&directory);
}

/// External source references survive the real MCP transport and first save.
#[test]
fn unsaved_external_import_survives_undo_save_and_reopen_over_stdio() {
    fn tool(client: &mut Client, name: &str, arguments: &Value) -> Value {
        let result = client.request("tools/call", &json!({"name": name, "arguments": arguments}));
        assert_ne!(result["isError"], json!(true), "{name}: {result}");
        result["structuredContent"].clone()
    }

    // Keep Unix socket paths below the macOS limit independently of TMPDIR.
    #[cfg(unix)]
    let base = PathBuf::from("/tmp");
    #[cfg(not(unix))]
    let base = std::env::temp_dir();
    let directory = base.join(format!("sub-mcp-import-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("endpoint directory");
    let media_dir = directory.join("source files");
    let save_dir = directory.join("saved elsewhere");
    std::fs::create_dir_all(&media_dir).unwrap();
    std::fs::create_dir_all(&save_dir).unwrap();
    let source = media_dir.join("one sample.wav");
    // A complete one-sample PCM WAV fixture; this test requires no decoder.
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&38_u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&96_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&2_u32.to_le_bytes());
    wav.extend_from_slice(&0_i16.to_le_bytes());
    std::fs::write(&source, wav).unwrap();
    let source = sub_model::plain_path(std::fs::canonicalize(source).unwrap());
    let expected_path = json!({"external": source});
    let engine = Engine::spawn(Project::new("Before handshake")).unwrap();
    let endpoint =
        Endpoint::in_directory(directory.clone(), INSTANCE).expect("a short native endpoint path");
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let server = Server::bind(endpoint, dispatcher).unwrap();
    let mut client = Client::start(&directory);
    client.initialize();
    tool(
        &mut client,
        "project_new",
        &json!({"name": "Unsaved import"}),
    );
    let mut item = serde_json::to_value(sub_model::MediaItem::new(
        sub_model::MediaPath::new("placeholder.wav").unwrap(),
    ))
    .unwrap();
    item["path"] = expected_path.clone();
    tool(&mut client, "media_import", &json!({"item": item}));
    let imported = engine.handle().snapshot().media[0].clone();
    assert_eq!(serde_json::to_value(&imported.path).unwrap(), expected_path);
    tool(&mut client, "edit_undo", &json!({}));
    assert!(engine.handle().snapshot().media.is_empty());
    tool(&mut client, "edit_redo", &json!({}));
    assert_eq!(engine.handle().snapshot().media[0], imported);
    tool(
        &mut client,
        "sequence_create",
        &json!({"name": "Main", "settings": SequenceSettings::default()}),
    );
    let sequence = engine.handle().snapshot().sequences[0].id;
    tool(
        &mut client,
        "track_add",
        &json!({"sequence": sequence, "name": "A1", "kind": "audio"}),
    );
    let track = engine.handle().snapshot().sequences[0].tracks[0].id;
    let start = sub_time::RationalTime::new(0, sub_time::Rational::FPS_24);
    let range = sub_time::TimeRange::new(
        start,
        sub_time::RationalTime::new(1, sub_time::Rational::FPS_24),
    )
    .unwrap();
    let clip = sub_model::Clip::new("Sample", imported.id, range);
    tool(
        &mut client,
        "clip_add",
        &json!({"sequence": sequence, "track": track, "start": start, "clip": clip}),
    );
    let expected = engine.handle().snapshot();
    let saved = save_dir.join("first-save.sub");
    tool(&mut client, "project_save", &json!({"path": saved}));
    let written: Value = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    assert_eq!(written["project"]["media"][0]["path"], expected_path);
    tool(&mut client, "project_new", &json!({"name": "Other"}));
    tool(&mut client, "project_open", &json!({"path": saved}));
    let reopened = engine.handle().snapshot();
    assert_eq!(*reopened, *expected);
    assert_eq!(reopened.media[0].absolute_path(&save_dir), source);
    assert!(reopened.media[0].absolute_path(&save_dir).is_file());
    drop(client);
    server.shutdown().unwrap();
    engine.shutdown().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}
