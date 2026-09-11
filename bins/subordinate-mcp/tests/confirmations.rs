//! Asking before something is destroyed, against a real editor (TASK-97).
//!
//! The three destructive tools — `sequence_delete`, `media_remove` and an
//! `export_render` that would write over a file already on disk — must not
//! reach the engine until someone has said yes. What is tested here is the
//! whole round trip: the first call answers `input_required` and changes
//! nothing, the retry carrying the user's answer either runs the call or fails
//! it with `mcp.confirmation_declined`, and a client with nobody to ask sets
//! `confirm` and is served in one go (docs/PLAN.md §7).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, InputRequiredResult};
use serde_json::{Value, json};
use sub_command::Dispatcher;
use sub_command::endpoint::Endpoint;
use sub_command::host::{ExportParams, ExportStatus, FrameImage, FrameRequest, Services};
use sub_command::transport::Server;
use sub_core::{SubError, SubResult, codes};
use sub_edit::Engine;
use sub_model::{
    Clip, MediaId, MediaItem, MediaPath, Project, Sequence, SequenceSettings, Track, TrackKind,
};
use sub_time::{RationalTime, TimeRange};
use subordinate_mcp::backend::{Backend, Options};
use subordinate_mcp::bridge::Bridge;
use subordinate_mcp::confirm::CONFIRM_KEY;
use subordinate_mcp::tools::ToolSet;

/// The instance this test serves, kept out of the way of a real editor.
const INSTANCE: &str = "mcp-confirm-test";

/// A stand-in for the encoder: an export here never writes anything, which is
/// exactly why the bridge has to decide about overwriting on its own.
#[derive(Debug, Default)]
struct FakeHost {
    exports: Mutex<BTreeMap<String, ExportStatus>>,
}

impl Services for FakeHost {
    fn project_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }

    fn probe(&self, _path: &Path) -> SubResult<Value> {
        Err(SubError::new(
            codes::UNIMPLEMENTED,
            "no decoders in this test",
        ))
    }

    fn make_proxy(&self, _project: &Project, _media: MediaId) -> SubResult<String> {
        Err(SubError::new(
            codes::UNIMPLEMENTED,
            "no decoders in this test",
        ))
    }

    fn render_frame_png(&self, _request: &FrameRequest<'_>) -> SubResult<FrameImage> {
        Err(SubError::new(codes::UNIMPLEMENTED, "no GPU in this test"))
    }

    fn presets(&self) -> SubResult<Vec<Value>> {
        Ok(vec![json!({ "id": "h264-mp4", "container": "mp4" })])
    }

    fn start_export(&self, _project: &Project, request: &ExportParams) -> SubResult<ExportStatus> {
        let status = ExportStatus::running("export-1", request.output.display().to_string(), 0, 48);
        self.exports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(status.job.clone(), status.clone());
        Ok(status)
    }

    fn export_progress(&self, job: &str) -> SubResult<ExportStatus> {
        self.exports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(job)
            .cloned()
            .ok_or_else(|| SubError::new(codes::NOT_FOUND, "no such export job"))
    }
}

/// A directory of this test's own for the socket, the lock file and the
/// export it pretends to write.
fn workspace() -> PathBuf {
    let directory = std::env::temp_dir().join(format!("sub-mcp-confirm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");
    directory
}

/// A project with one media item and two sequences, one of them expendable.
fn fixture() -> Project {
    let mut project = Project::new("Doc cut");
    let item = MediaItem::new(MediaPath::new("media/shot.mov").expect("a relative path"));
    let media = item.id;
    project.media.push(item);

    for name in ["Main", "Offcuts"] {
        let mut sequence = Sequence::new(name, SequenceSettings::default());
        let rate = sequence.settings.frame_rate;
        let mut track = Track::new("V1", TrackKind::Video);
        let source = TimeRange::new(RationalTime::new(0, rate), RationalTime::new(48, rate))
            .expect("a forty-eight frame source range");
        track.items.push(Clip::new("shot", media, source).into());
        sequence.tracks.push(track);
        project.sequences.push(sequence);
    }
    project
}

/// A call to one tool, with its arguments.
fn call(name: &str, arguments: &Value) -> CallToolRequestParams {
    let mut request = CallToolRequestParams::new(name.to_owned());
    request.arguments = arguments.as_object().cloned();
    request
}

/// The same call again, carrying the user's answer and the state the bridge
/// handed out — which is what a client does after its dialog closes.
fn retry(
    name: &str,
    arguments: &Value,
    asked: &InputRequiredResult,
    answer: &Value,
) -> CallToolRequestParams {
    let mut request = call(name, arguments);
    request.request_state.clone_from(&asked.request_state);
    request.input_responses = Some(
        [(CONFIRM_KEY.to_owned(), answer.clone())]
            .into_iter()
            .collect(),
    );
    request
}

/// A call the bridge answered with a question.
fn asks(bridge: &Bridge, request: CallToolRequestParams) -> InputRequiredResult {
    match bridge.call(request).expect("the tool is routed") {
        CallToolResponse::InputRequired(asked) => asked,
        other => panic!("the call did not ask first: {other:?}"),
    }
}

/// A call the bridge ran.
fn completes(bridge: &Bridge, request: CallToolRequestParams) -> CallToolResult {
    match bridge.call(request).expect("the tool is routed") {
        CallToolResponse::Complete(result) => result,
        other => panic!("the call did not complete: {other:?}"),
    }
}

/// The user accepting the question.
fn accepted() -> Value {
    json!({ "action": "accept", "content": { "confirm": true } })
}

#[test]
fn destructive_tools_ask_before_they_destroy_anything() {
    let directory = workspace();
    let engine = Engine::spawn(fixture()).expect("an engine");
    let endpoint = Endpoint::in_directory(directory.clone(), INSTANCE).expect("an endpoint");
    let mut dispatcher = Dispatcher::new(engine.handle().clone());
    sub_command::host::register_methods(&mut dispatcher, Arc::new(FakeHost::default()))
        .expect("the host-served families");
    let server = Server::bind(endpoint, Arc::new(dispatcher)).expect("a bound server");

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
    let snapshot = engine.handle().snapshot();
    let doomed = snapshot.sequences[1].id;
    let kept = snapshot.sequences[0].id;
    let media = snapshot.media[0].id;

    // Every destructive tool takes the argument that skips the round trip,
    // and its Command API method does not.
    for tool in ["sequence_delete", "media_remove", "export_render"] {
        let published = bridge
            .tools()
            .tools()
            .iter()
            .find(|published| published.name == tool)
            .unwrap_or_else(|| panic!("{tool} is published"));
        assert_eq!(
            published.input_schema["properties"]["confirm"]["type"], "boolean",
            "{tool} does not declare the confirmation argument",
        );
    }

    // 1. A deletion that was not confirmed asks, and nothing happens yet.
    let arguments = json!({ "sequence": doomed });
    let asked = asks(&bridge, call("sequence_delete", &arguments));
    let requests = asked.input_requests.as_ref().expect("an elicitation");
    let elicitation = requests.get(CONFIRM_KEY).expect("the confirmation");
    let wire = serde_json::to_value(elicitation).expect("an elicitation request");
    assert_eq!(wire["method"], "elicitation/create");
    assert!(
        wire["params"]["message"]
            .as_str()
            .expect("a message")
            .contains(&doomed.to_string()),
        "the question must name the sequence: {wire}",
    );
    assert!(asked.request_state.is_some(), "the retry needs a state");
    assert_eq!(engine.handle().snapshot().sequences.len(), 2);

    // 2. The same call, retried with the user's yes, is the one that runs.
    let deleted = completes(
        &bridge,
        retry("sequence_delete", &arguments, &asked, &accepted()),
    );
    assert_eq!(deleted.is_error, Some(false), "{deleted:?}");
    let left = engine.handle().snapshot();
    assert_eq!(left.sequences.len(), 1);
    assert_eq!(left.sequences[0].id, kept);

    // 3. A no fails the call with a stable code, and touches nothing.
    for answer in [
        json!({ "action": "decline" }),
        json!({ "action": "cancel" }),
        json!({ "action": "accept", "content": { "confirm": false } }),
    ] {
        let arguments = json!({ "media": media, "force": true });
        let asked = asks(&bridge, call("media_remove", &arguments));
        let refused = completes(&bridge, retry("media_remove", &arguments, &asked, &answer));
        assert_eq!(refused.is_error, Some(true), "{answer} must refuse");
        let structured = refused.structured_content.expect("structured content");
        assert_eq!(structured["code"], "mcp.confirmation_declined");
        assert_eq!(structured["details"]["tool"], "media_remove");
        assert_eq!(engine.handle().snapshot().media.len(), 1);
    }

    // 4. A client with nobody to ask says so itself, in one round trip.
    let removed = completes(
        &bridge,
        call(
            "media_remove",
            &json!({ "media": media, "force": true, "confirm": true }),
        ),
    );
    assert_eq!(removed.is_error, Some(false), "{removed:?}");
    assert!(engine.handle().snapshot().media.is_empty());

    drop(bridge);
    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
}

#[test]
fn an_export_asks_only_when_it_would_overwrite() {
    let directory = std::env::temp_dir().join(format!("sub-mcp-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");
    let engine = Engine::spawn(fixture()).expect("an engine");
    let endpoint =
        Endpoint::in_directory(directory.clone(), "mcp-confirm-export-test").expect("an endpoint");
    let mut dispatcher = Dispatcher::new(engine.handle().clone());
    sub_command::host::register_methods(&mut dispatcher, Arc::new(FakeHost::default()))
        .expect("the host-served families");
    let server = Server::bind(endpoint, Arc::new(dispatcher)).expect("a bound server");

    let backend = Backend::connect(&Options {
        instance: "mcp-confirm-export-test".to_owned(),
        directory: Some(directory.clone()),
        ..Options::default()
    })
    .expect("the bridge finds the editor");
    let bridge = Bridge::new(
        Arc::new(ToolSet::committed().expect("the committed tool set")),
        Arc::new(backend),
    );

    // A path with nothing at it destroys nothing, so it is not asked about.
    let fresh = directory.join("first-cut.mp4");
    let started = completes(
        &bridge,
        call(
            "export_render",
            &json!({ "preset": "h264-mp4", "output": fresh }),
        ),
    );
    assert_eq!(started.is_error, Some(false), "{started:?}");

    // The same path, once something is there, is an overwrite.
    std::fs::write(&fresh, b"an earlier render").expect("a file to overwrite");
    let arguments = json!({ "preset": "h264-mp4", "output": fresh });
    let asked = asks(&bridge, call("export_render", &arguments));
    let message = serde_json::to_value(
        asked
            .input_requests
            .as_ref()
            .expect("an elicitation")
            .get(CONFIRM_KEY)
            .expect("the confirmation"),
    )
    .expect("an elicitation request")["params"]["message"]
        .as_str()
        .expect("a message")
        .to_owned();
    assert!(message.contains("first-cut.mp4"), "{message}");
    assert!(message.contains("cannot be undone"), "{message}");

    let overwritten = completes(
        &bridge,
        retry("export_render", &arguments, &asked, &accepted()),
    );
    assert_eq!(overwritten.is_error, Some(false), "{overwritten:?}");
    assert_eq!(
        overwritten.structured_content.expect("structured content")["state"],
        "running",
    );

    drop(bridge);
    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
    let _ = std::fs::remove_dir_all(&directory);
}
