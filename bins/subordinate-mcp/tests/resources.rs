//! Resources against a real engine.
//!
//! The bridge's resources are a projection of the project the editor has open,
//! so the only test worth writing runs a real engine behind a real Command API
//! socket: list what it offers, read one of each kind, and see an edit come
//! back down the change feed as the resource it made stale.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ResourceContents};
use serde_json::{Value, json};
use sub_command::Dispatcher;
use sub_command::endpoint::Endpoint;
use sub_command::transport::Server;
use sub_edit::Engine;
use sub_model::{MediaItem, MediaPath, Project, Sequence, SequenceSettings};
use subordinate_mcp::backend::{Backend, Options};
use subordinate_mcp::resources::{PROJECT_URI, media_uri, sequence_uri};

/// The instance these tests serve, kept out of the way of a real editor.
const INSTANCE: &str = "mcp-resources-test";

/// How long a test waits for an update that should be immediate.
const PATIENCE: Duration = Duration::from_secs(5);

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

/// A project with one sequence and one media item to project into resources.
fn project() -> Project {
    let mut project = Project::new("Doc cut");
    project
        .sequences
        .push(Sequence::new("Timeline 1", SequenceSettings::default()));
    project.media.push(MediaItem::new(
        MediaPath::new("footage/interview.mp4").expect("a relative media path"),
    ));
    project
}

/// A call to one tool, with its arguments.
fn call(name: &str, arguments: &Value) -> CallToolRequestParams {
    let mut request = CallToolRequestParams::new(name.to_owned());
    request.arguments = Some(
        arguments
            .as_object()
            .expect("arguments are an object")
            .clone(),
    );
    request
}

/// One tool call, run to completion.
///
/// A destructive tool answers `input_required` before it runs anything (see
/// `tests/confirmations.rs`); nothing called here is one, so every call
/// finishes in a single round trip.
fn run(
    bridge: &subordinate_mcp::Bridge,
    request: CallToolRequestParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    bridge.call(request).map(|response| match response {
        CallToolResponse::Complete(result) => result,
        other => panic!("the call did not complete: {other:?}"),
    })
}

/// The JSON one resource read carries.
fn contents(result: &rmcp::model::ReadResourceResult) -> Value {
    let [
        ResourceContents::TextResourceContents {
            text, mime_type, ..
        },
    ] = result.contents.as_slice()
    else {
        panic!("a resource is one piece of text");
    };
    assert_eq!(mime_type.as_deref(), Some("application/json"));
    serde_json::from_str(text).expect("a resource is JSON")
}

#[test]
fn the_project_its_sequences_and_its_media_are_readable_resources() {
    let directory = workspace("resources");
    let project = project();
    let sequence = project.sequences[0].id.to_string();
    let media = project.media[0].id.to_string();

    let engine = Engine::spawn(project).expect("an engine");
    let endpoint = Endpoint::in_directory(directory.clone(), INSTANCE).expect("an endpoint");
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let server = Server::bind(endpoint, dispatcher).expect("a bound server");

    let backend = Backend::connect(&options(&directory)).expect("the bridge finds the editor");
    let bridge = subordinate_mcp::Bridge::new(
        Arc::new(subordinate_mcp::ToolSet::committed().expect("the committed tool set")),
        Arc::new(backend),
    );
    let resources = bridge.resources();

    // The list is the project and one resource per entity.
    let listed = resources.list().expect("the project can be listed");
    let uris: Vec<&str> = listed.iter().map(|entry| entry.uri.as_str()).collect();
    assert_eq!(
        uris,
        [
            PROJECT_URI,
            sequence_uri(&sequence).as_str(),
            media_uri(&media).as_str(),
        ],
    );

    // Each one reads back as the project file's own JSON, with the revision.
    let whole = resources.read(PROJECT_URI).expect("the project reads");
    let json = contents(&whole);
    assert_eq!(json["revision"], 0);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["project"]["name"], "Doc cut");
    assert_eq!(json["project"]["sequences"][0]["id"], sequence);

    let timeline = resources
        .read(&sequence_uri(&sequence))
        .expect("the sequence reads");
    let json = contents(&timeline);
    assert_eq!(json["sequence"]["name"], "Timeline 1");
    assert_eq!(json["sequence"]["settings"]["frame_rate"]["numerator"], 24);

    let item = resources.read(&media_uri(&media)).expect("the media reads");
    let json = contents(&item);
    assert_eq!(json["media"]["name"], "interview.mp4");
    assert_eq!(json["media"]["path"], "footage/interview.mp4");

    // Caching hints travel with every read (SEP-2549).
    assert_eq!(
        whole.ttl_ms,
        Some(subordinate_mcp::resources::RESOURCE_TTL_MS),
    );
    assert_eq!(whole.cache_scope, Some(rmcp::model::CacheScope::Private));

    // A URI naming nothing carries the bridge's own stable code.
    for uri in [
        "sequence://absent",
        "media://absent",
        "clip://c1",
        "project://someone-elses",
    ] {
        let error = resources.read(uri).expect_err("no such resource");
        assert_eq!(error.code.as_str(), "mcp.unknown_resource", "{uri}");
    }

    drop(bridge);
    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
}

#[test]
fn an_edit_reaches_the_change_feed_as_the_resources_it_made_stale() {
    let directory = workspace("watch");
    let project = project();
    let sequence = sequence_uri(&project.sequences[0].id.to_string());

    let engine = Engine::spawn(project).expect("an engine");
    let endpoint = Endpoint::in_directory(directory.clone(), INSTANCE).expect("an endpoint");
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let server = Server::bind(endpoint, dispatcher).expect("a bound server");

    let backend = Backend::connect(&options(&directory)).expect("the bridge finds the editor");
    let bridge = subordinate_mcp::Bridge::new(
        Arc::new(subordinate_mcp::ToolSet::committed().expect("the committed tool set")),
        Arc::new(backend),
    );

    // Watching is a second connection of its own, subscribed to the event bus.
    let mut updates = bridge.watch().updates().expect("the editor can be watched");
    assert!(bridge.watch().is_watching());

    // A track added to the sequence stales the project and the timeline.
    let added = run(
        &bridge,
        call(
            "track_add",
            &json!({
                "sequence": sequence.trim_start_matches("sequence://"),
                "name": "V1",
                "kind": "video",
            }),
        ),
    )
    .expect("track.add is a tool");
    assert_eq!(added.is_error, Some(false), "{added:?}");

    let deadline = Instant::now() + PATIENCE;
    let update = loop {
        assert!(Instant::now() < deadline, "no update arrived");
        let update = updates.blocking_recv().expect("the change feed stays open");
        if update.revision > 0 {
            break update;
        }
    };
    assert!(update.touches(PROJECT_URI));
    assert!(update.touches(&sequence));
    assert!(!update.touches("media://anything"));

    // A new media item changes the set of resources, not only their contents.
    run(&bridge, call("bin_create", &json!({ "name": "Footage" }))).expect("bin.create is a tool");
    let update = updates.blocking_recv().expect("the change feed stays open");
    assert!(update.touches(PROJECT_URI));
    assert!(!update.list_changed, "a bin is not a listed resource");

    drop(bridge);
    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
}
