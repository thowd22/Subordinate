//! Every tool the bridge publishes, called once against a real editor.
//!
//! docs/PLAN.md §7 names five families an agent works through — `project.*`,
//! `media.*`, `timeline.*`, `playback.*` and `export.*` — beside the plugin
//! family and the entity-shaped command set. This test is the whole surface at
//! once:
//!
//! 1. the families are exercised with real arguments against a real engine, so
//!    what they claim to do is what happens to the project; and
//! 2. every published tool, without exception, is called once and has to come
//!    back as a tool result — either a success or the structured `SubError`
//!    with its stable code. A tool that answered a *protocol* error would be
//!    one the bridge publishes but cannot route, which is the failure this
//!    catches.
//!
//! The host-served tools (`media.probe`, `media.make_proxy`,
//! `playback.render_frame_png`, `export.*`) need decoders, a GPU and an
//! encoder, none of which a test machine is owed, so the editor here is served
//! with a stand-in [`sub_command::host::Services`]. What is under test is the
//! bridge and the Command API surface, not GStreamer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use serde_json::{Value, json};
use sub_command::Dispatcher;
use sub_command::endpoint::Endpoint;
use sub_command::host::{ExportParams, ExportStatus, FrameImage, FrameRequest, Services};
use sub_command::transport::Server;
use sub_core::{SubError, SubResult, codes};
use sub_edit::{Engine, EngineHandle};
use sub_model::{
    Clip, ClipId, MediaId, MediaItem, MediaPath, Project, Sequence, SequenceId, SequenceSettings,
    Track, TrackId, TrackKind,
};
use sub_time::{Rational, RationalTime, TimeRange};
use subordinate_mcp::backend::{Backend, Options};
use subordinate_mcp::bridge::Bridge;
use subordinate_mcp::tools::ToolSet;

/// The instance this test serves, kept out of the way of a real editor.
const INSTANCE: &str = "mcp-families-test";

/// A stand-in for the decoders, the GPU and the encoder.
#[derive(Debug, Default)]
struct FakeHost {
    exports: Mutex<HashMap<String, ExportStatus>>,
}

impl Services for FakeHost {
    fn project_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }

    fn probe(&self, path: &Path) -> SubResult<Value> {
        Ok(json!({
            "path": path.display().to_string(),
            "container": "video/quicktime",
            "video": [{ "width": 1920, "height": 1080 }],
            "audio": [],
        }))
    }

    fn make_proxy(&self, _project: &Project, media: MediaId) -> SubResult<String> {
        Ok(format!("proxies/{media}.mov"))
    }

    fn render_frame_png(&self, request: &FrameRequest<'_>) -> SubResult<FrameImage> {
        // A one-pixel PNG: the bridge only has to carry it, not decode it.
        Ok(FrameImage {
            data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGMAAQAABQAB\
                   h6FO1AAAAABJRU5ErkJggg=="
                .to_owned(),
            mime_type: "image/png".to_owned(),
            width: request.width.unwrap_or(1920),
            height: 1080,
            time: request.time,
        })
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

/// A directory of this test's own for the socket and the lock file.
fn workspace() -> PathBuf {
    let directory = std::env::temp_dir().join(format!("sub-mcp-families-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");
    directory
}

/// A project with one media item and one sequence holding one clip.
fn fixture() -> Project {
    let mut project = Project::new("Doc cut");
    let item = MediaItem::new(MediaPath::new("media/shot.mov").expect("a relative path"));
    let media = item.id;
    project.media.push(item);

    let mut sequence = Sequence::new("Main", SequenceSettings::default());
    let rate = sequence.settings.frame_rate;
    let mut track = Track::new("V1", TrackKind::Video);
    let source = TimeRange::new(RationalTime::new(0, rate), RationalTime::new(48, rate))
        .expect("a forty-eight frame source range");
    track.items.push(Clip::new("shot", media, source).into());
    sequence.tracks.push(track);
    project.sequences.push(sequence);
    project
}

/// A call to one tool, with its arguments.
fn call(name: &str, arguments: &Value) -> CallToolRequestParams {
    let mut request = CallToolRequestParams::new(name.to_owned());
    request.arguments = arguments.as_object().cloned();
    request
}

/// The structured JSON a tool answered with.
fn structured(result: &CallToolResult) -> Value {
    result
        .structured_content
        .clone()
        .unwrap_or_else(|| panic!("a tool answered without structured content"))
}

/// Runs one tool and requires it to succeed.
fn ok(bridge: &Bridge, name: &str, arguments: &Value) -> Value {
    let result = bridge
        .call(call(name, arguments))
        .unwrap_or_else(|error| panic!("{name} is not routed: {error}"));
    assert!(
        result.is_error != Some(true),
        "{name} failed: {:?}",
        result.structured_content,
    );
    structured(&result)
}

#[test]
fn every_published_tool_is_called_once_and_the_families_do_what_they_say() {
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
    let what = What {
        sequence: snapshot.sequences[0].id,
        track: snapshot.sequences[0].tracks[0].id,
        clip: snapshot.sequences[0].tracks[0].items[0]
            .as_clip()
            .expect("the fixture clip")
            .id,
        media: snapshot.media[0].id,
        rate: snapshot.sequences[0].settings.frame_rate,
    };
    let engine_handle = engine.handle().clone();

    project_family(&bridge, &engine_handle, &directory);
    media_family(&bridge, &engine_handle, &what);
    timeline_family(&bridge, &engine_handle, &what);
    playback_family(&bridge, &what);
    export_family(&bridge, &directory);
    let called = every_tool_once(&bridge);
    assert_eq!(called, bridge.tools().len());
    assert!(called > 100, "the whole Command API is published: {called}");

    server.shutdown().expect("the server stops");
    engine.shutdown().expect("the engine stops");
    let _ = std::fs::remove_dir_all(&directory);
}

/// What the fixture project holds, so each family can address it.
struct What {
    sequence: SequenceId,
    track: TrackId,
    clip: ClipId,
    media: MediaId,
    rate: Rational,
}

impl What {
    /// A time in frames of the fixture sequence's timebase.
    fn time(&self, frames: i64) -> Value {
        json!(RationalTime::new(frames, self.rate))
    }
}

/// `project.*`: listing, settings, and the three that swap the open document.
fn project_family(bridge: &Bridge, engine: &EngineHandle, directory: &Path) {
    let sequences = ok(bridge, "project_list_sequences", &json!({}));
    assert_eq!(sequences["sequences"][0]["name"], "Main");
    assert_eq!(sequences["sequences"][0]["tracks"], 1);
    let settings = ok(bridge, "project_settings", &json!({}));
    assert_eq!(settings["name"], "Doc cut");
    assert_eq!(settings["media_count"], 1);

    let saved = directory.join("saved.sub");
    let written = ok(bridge, "project_save", &json!({ "path": saved }));
    assert!(written["bytes"].as_u64().unwrap_or(0) > 0);
    assert!(saved.is_file(), "the project file was written");

    ok(bridge, "project_new", &json!({ "name": "Scratch" }));
    assert_eq!(engine.snapshot().name, "Scratch");
    ok(bridge, "project_open", &json!({ "path": saved }));
    assert_eq!(engine.snapshot().name, "Doc cut");
}

/// `media.*`: what the project references, what a file holds, and a proxy.
fn media_family(bridge: &Bridge, engine: &EngineHandle, what: &What) {
    let listed = ok(bridge, "media_list", &json!({}));
    assert_eq!(listed["media"].as_array().map(Vec::len), Some(1));
    let probed = ok(bridge, "media_probe", &json!({ "media": what.media }));
    assert_eq!(probed["container"], "video/quicktime");
    ok(bridge, "media_make_proxy", &json!({ "media": what.media }));
    assert!(
        engine.snapshot().media[0].proxy.path().is_some(),
        "the proxy was recorded on the item",
    );
}

/// `timeline.*`: the state query, and aliases that are real command steps.
fn timeline_family(bridge: &Bridge, engine: &EngineHandle, what: &What) {
    let (sequence, track, clip, rate) = (what.sequence, what.track, what.clip, what.rate);
    let state = ok(bridge, "timeline_get_state", &json!({}));
    assert_eq!(state["sequence"]["name"], "Main");
    assert!(state["duration"].is_object(), "an exact rational duration");

    ok(
        bridge,
        "timeline_add_track",
        &json!({ "sequence": sequence, "kind": "video", "name": "V2" }),
    );
    assert_eq!(engine.snapshot().sequences[0].tracks.len(), 2);
    ok(
        bridge,
        "timeline_add_marker",
        &json!({
            "target": { "on": "sequence", "sequence": sequence },
            "marker": sub_model::Marker::new(
                "Beat",
                TimeRange::new(RationalTime::new(12, rate), RationalTime::new(1, rate))
                    .expect("a one-frame marker"),
            ),
        }),
    );
    ok(
        bridge,
        "timeline_split_clip",
        &json!({
            "sequence": sequence,
            "track": track,
            "clip": clip,
            "at": what.time(24),
        }),
    );
    assert_eq!(
        engine.snapshot().sequences[0].tracks[0].items.len(),
        2,
        "the split left two clips",
    );
    // An alias is one history step of the command it names.
    ok(bridge, "edit_undo", &json!({}));
    assert_eq!(engine.snapshot().sequences[0].tracks[0].items.len(), 1);
}

/// `playback.*`: the transport, and the frame an agent looks at.
fn playback_family(bridge: &Bridge, what: &What) {
    let playing = ok(
        bridge,
        "playback_play",
        &json!({ "speed": "forward1x", "sequence": what.sequence }),
    );
    assert_eq!(playing["playing"], true);
    let sought = ok(
        bridge,
        "playback_seek",
        &json!({ "position": what.time(12) }),
    );
    assert_eq!(sought["position"]["value"], 12);
    let paused = ok(bridge, "playback_pause", &json!({}));
    assert_eq!(paused["playing"], false);
    ok(bridge, "playback_status", &json!({}));

    // A frame comes back as something a model can look at, not only as JSON.
    let frame = bridge
        .call(call(
            "playback_render_frame_png",
            &json!({ "time": what.time(12), "width": 320 }),
        ))
        .expect("the tool is routed");
    assert!(
        frame.is_error != Some(true),
        "{:?}",
        frame.structured_content
    );
    assert_eq!(structured(&frame)["mime_type"], "image/png");
    let image = frame
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Image(image) => Some(image),
            _ => None,
        })
        .expect("the frame is published as an image content block");
    assert_eq!(image.mime_type, "image/png");
    assert!(!image.data.is_empty());
}

/// `export.*`: the presets, a job, and following it.
fn export_family(bridge: &Bridge, directory: &Path) {
    let presets = ok(bridge, "export_list_presets", &json!({}));
    assert_eq!(presets["presets"][0]["id"], "h264-mp4");
    let started = ok(
        bridge,
        "export_render",
        &json!({ "preset": "h264-mp4", "output": directory.join("out.mp4") }),
    );
    assert_eq!(started["state"], "running");
    let job = started["job"].as_str().expect("a job id").to_owned();
    let progress = ok(bridge, "export_progress", &json!({ "job": job }));
    assert_eq!(progress["frames_total"], 48);
}

/// Calls every published tool once, and returns how many there were.
///
/// Whatever the families above did not reach is reached here. A tool may
/// refuse the arguments it is given — an empty object is not valid for most
/// commands — but it must answer as a tool result carrying a stable error
/// code, never as a protocol error, which is what a published-but-unroutable
/// tool would produce.
fn every_tool_once(bridge: &Bridge) -> usize {
    let mut called = 0usize;
    for tool in bridge.tools().tools() {
        let name = tool.name.to_string();
        let result = bridge
            .call(call(&name, &json!({})))
            .unwrap_or_else(|error| panic!("{name} is published but not routed: {error}"));
        called += 1;
        assert!(
            !result.content.is_empty(),
            "{name} answered with no content",
        );
        let answer = structured(&result);
        if result.is_error == Some(true) {
            let code = answer["code"].as_str().unwrap_or_else(|| {
                panic!("{name} failed without a stable code: {answer}");
            });
            assert!(
                sub_core::ErrorCode::parse(code).is_ok(),
                "{name} failed with a malformed code: {code}",
            );
        }
    }
    called
}
