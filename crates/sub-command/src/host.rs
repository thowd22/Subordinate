//! The tool families that need more than the engine (docs/PLAN.md §7).
//!
//! [`crate::agent`] holds the part of `project.*`, `media.*`, `timeline.*` and
//! `playback.*` the engine can answer on its own. The rest of the agent
//! surface cannot be served from project state alone:
//!
//! - `media.probe` reads a file with the GStreamer discoverer.
//! - `media.make_proxy` transcodes one.
//! - `playback.render_frame_png` composites a frame on a GPU and encodes it,
//!   which is what lets an agent *look* at the timeline rather than read it.
//! - `export.list_presets`, `export.render` and `export.progress` drive the
//!   encoder.
//!
//! Those all belong to the process that is serving — the editor, or
//! `subordinate-cli serve` — so this module defines what such a process must
//! provide ([`Services`]) and nothing else. A build with no GPU or no
//! GStreamer simply never calls [`register_methods`], and the bridge publishes
//! the families the engine serves; it does not publish tools that cannot run.
//!
//! Like the plugin management methods, these are exported as their own schema
//! document ([`schema`], committed at `docs/schema/host-api.json`) because they
//! are not on every dispatcher, and the MCP bridge compiles that document in
//! beside the other two.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sub_core::{SubError, SubResult, codes};
use sub_edit::commands::SetProxyState;
use sub_model::{MediaId, MediaPath, Project, ProxyState, SequenceId};
use sub_time::RationalTime;

use crate::agent::pick_sequence;
use crate::dispatch::{AppliedResult, Dispatcher, NoParams, to_value, typed};

/// Read a media file's streams without importing it.
pub const MEDIA_PROBE: &str = "media.probe";
/// Transcode a low-resolution stand-in for a media item.
pub const MEDIA_MAKE_PROXY: &str = "media.make_proxy";
/// Composite one frame and hand it back as a PNG.
pub const PLAYBACK_RENDER_FRAME_PNG: &str = "playback.render_frame_png";
/// List the export presets this build offers.
pub const EXPORT_LIST_PRESETS: &str = "export.list_presets";
/// Start an export.
pub const EXPORT_RENDER: &str = "export.render";
/// Read where an export has got to.
pub const EXPORT_PROGRESS: &str = "export.progress";

/// What a serving process must provide for the host-backed families.
///
/// One implementation per process: the editor's and the headless CLI's differ
/// only in where the pictures are drawn.
pub trait Services: Send + Sync + fmt::Debug + 'static {
    /// The folder the open project file lives in.
    ///
    /// Media paths are stored project-relative (docs/PLAN.md §5.6), so this is
    /// what turns one back into a file a decoder can open.
    fn project_dir(&self) -> PathBuf;

    /// Resolve media against the file associated with this project snapshot.
    ///
    /// # Errors
    /// Returns an error when the project has no known file context.
    fn project_dir_for(&self, _project: &Project) -> SubResult<PathBuf> {
        Ok(self.project_dir())
    }

    /// Remember a successful project open/save, or a newly unsaved project.
    fn project_file_changed(&self, _project: &Project, _path: Option<&Path>) {}

    /// Reads a media file's streams.
    ///
    /// # Errors
    ///
    /// Whatever the prober returns for a file it cannot open.
    fn probe(&self, path: &Path) -> SubResult<Value>;

    /// Transcodes a proxy for one media item, returning its project-relative
    /// path.
    ///
    /// # Errors
    ///
    /// Whatever the proxy pipeline returns, and `core.not_found` when the
    /// project references no such item.
    fn make_proxy(&self, project: &Project, media: MediaId) -> SubResult<String>;

    /// Composites one frame of a sequence and encodes it as a PNG.
    ///
    /// # Errors
    ///
    /// Whatever the compositor, the decoders or the encoder return.
    fn render_frame_png(&self, request: &FrameRequest<'_>) -> SubResult<FrameImage>;

    /// The export presets this build offers.
    ///
    /// # Errors
    ///
    /// Whatever the preset library returns for a malformed presets file.
    fn presets(&self) -> SubResult<Vec<Value>>;

    /// Starts an export and returns its first status.
    ///
    /// # Errors
    ///
    /// Whatever the preset library, the encoder probe and the pipeline return.
    fn start_export(&self, project: &Project, request: &ExportParams) -> SubResult<ExportStatus>;

    /// Reads where an export has got to.
    ///
    /// # Errors
    ///
    /// `core.not_found` when no export by that id is known to this process.
    fn export_progress(&self, job: &str) -> SubResult<ExportStatus>;
}

/// Everything a single-frame render needs, resolved.
#[derive(Debug, Clone, Copy)]
pub struct FrameRequest<'a> {
    /// The project the sequence belongs to.
    pub project: &'a Project,
    /// The sequence to draw.
    pub sequence: SequenceId,
    /// The exact time on that sequence's timeline.
    pub time: RationalTime,
    /// The width the picture is scaled to, or the canvas width when absent.
    pub width: Option<u32>,
}

/// Puts the host-backed families on `dispatcher`.
///
/// # Errors
///
/// `command.duplicate_method` when one of the names is already served, which
/// would mean this was installed twice.
pub fn register_methods(dispatcher: &mut Dispatcher, services: Arc<dyn Services>) -> SubResult<()> {
    *dispatcher
        .host_services
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&services));
    let host = Arc::clone(&services);
    dispatcher.register::<ProbeParams, Value, _>(
        MEDIA_PROBE,
        "Read a media file's video and audio streams without importing it.",
        move |engine, params| {
            let params: ProbeParams = typed(params)?;
            let path = match (params.path, params.media) {
                (Some(path), None) => path,
                (None, Some(media)) => {
                    let project = engine.snapshot();
                    media_path(&project, &host.project_dir_for(&project)?, media)?
                }
                _ => {
                    return Err(SubError::new(
                        codes::INVALID_ARGUMENT,
                        "exactly one of path and media must be given",
                    ));
                }
            };
            host.probe(&path)
        },
    )?;

    let host = Arc::clone(&services);
    dispatcher.register::<ProxyParams, AppliedResult, _>(
        MEDIA_MAKE_PROXY,
        "Transcode a low-resolution proxy for a media item and record it on the item.",
        move |engine, params| {
            let params: ProxyParams = typed(params)?;
            let project = engine.snapshot();
            let path = host.make_proxy(&project, params.media)?;
            let applied = engine.apply(SetProxyState::new(
                params.media,
                ProxyState::Ready(MediaPath::new(&path)?),
            ))?;
            to_value(&AppliedResult::from(&applied))
        },
    )?;

    let host = Arc::clone(&services);
    dispatcher.register::<FrameParams, FrameImage, _>(
        PLAYBACK_RENDER_FRAME_PNG,
        "Composite one frame of a sequence and return it as a PNG, so an agent can look at it.",
        move |engine, params| {
            let params: FrameParams = typed(params)?;
            let project = engine.snapshot();
            let sequence = pick_sequence(&project, params.sequence)?.id;
            let image = host.render_frame_png(&FrameRequest {
                project: &project,
                sequence,
                time: params.time,
                width: params.width,
            })?;
            to_value(&image)
        },
    )?;

    let host = Arc::clone(&services);
    dispatcher.register::<NoParams, PresetListResult, _>(
        EXPORT_LIST_PRESETS,
        "List the export presets this build offers, with their container and codecs.",
        move |_, params| {
            typed::<NoParams>(params)?;
            to_value(&PresetListResult {
                presets: host.presets()?,
            })
        },
    )?;

    let host = Arc::clone(&services);
    dispatcher.register::<ExportParams, ExportStatus, _>(
        EXPORT_RENDER,
        "Render a sequence to a file with an export preset, reporting the job that does it.",
        move |engine, params| {
            let params: ExportParams = typed(params)?;
            let project = engine.snapshot();
            to_value(&host.start_export(&project, &params)?)
        },
    )?;

    dispatcher.register::<ProgressParams, ExportStatus, _>(
        EXPORT_PROGRESS,
        "Read how far an export has got, and whether it finished or failed.",
        move |_, params| {
            let params: ProgressParams = typed(params)?;
            to_value(&services.export_progress(&params.job)?)
        },
    )?;
    Ok(())
}

/// The absolute path of a media item's source file.
fn media_path(project: &Project, project_dir: &Path, media: MediaId) -> SubResult<PathBuf> {
    project.absolute_path(project_dir, media).ok_or_else(|| {
        SubError::new(codes::NOT_FOUND, "no such media item in the project")
            .with_detail("media", media.to_string())
    })
}

/// The parameters of [`MEDIA_PROBE`]: exactly one of the two.
#[derive(Debug, Clone, Default, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeParams {
    /// A file to read, named directly.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// A media item of the open project to read.
    #[serde(default)]
    pub media: Option<MediaId>,
}

/// The parameters of [`MEDIA_MAKE_PROXY`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyParams {
    /// The media item to make a proxy for.
    pub media: MediaId,
}

/// The parameters of [`PLAYBACK_RENDER_FRAME_PNG`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameParams {
    /// The exact time on the sequence's timeline to draw.
    pub time: RationalTime,
    /// Which sequence; absent takes the project's only one.
    #[serde(default)]
    pub sequence: Option<SequenceId>,
    /// The width to scale the picture to; absent draws at the canvas width.
    #[serde(default)]
    pub width: Option<u32>,
}

/// One frame, encoded.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct FrameImage {
    /// The PNG itself, base64-encoded, as an MCP image content block carries
    /// it.
    pub data: String,
    /// Always `image/png`.
    pub mime_type: String,
    /// The picture's width in pixels.
    pub width: u32,
    /// The picture's height in pixels.
    pub height: u32,
    /// The time that was drawn.
    pub time: RationalTime,
}

impl FrameImage {
    /// A PNG of `width` by `height` pixels drawn at `time`, encoded for the
    /// wire.
    ///
    /// Every serving process answers `playback.render_frame_png` with this, so
    /// the base64 and the media type are written once rather than once per
    /// front end.
    #[must_use]
    pub fn png(png: &[u8], width: u32, height: u32, time: RationalTime) -> Self {
        Self {
            data: base64(png),
            mime_type: "image/png".to_owned(),
            width,
            height,
            time,
        }
    }
}

/// Standard base64, as an MCP image content block carries a picture.
///
/// Written out here rather than taken from a crate: it is twenty lines, it is
/// the only encoding this workspace needs, and both serving processes — the
/// editor and `subordinate-cli serve` — answer with it.
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let triple = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                let shift = 18 - index * 6;
                out.push(char::from(ALPHABET[((triple >> shift) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The result of [`EXPORT_LIST_PRESETS`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct PresetListResult {
    /// The presets, in the library's order.
    pub presets: Vec<Value>,
}

/// The frames an export covers, at the sequence's own timebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRange {
    /// The first frame written.
    pub start_frame: i64,
    /// One past the last frame written; absent runs to the end.
    #[serde(default)]
    pub end_frame: Option<i64>,
}

/// The parameters of [`EXPORT_RENDER`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportParams {
    /// The export preset, by id, as [`EXPORT_LIST_PRESETS`] names it.
    pub preset: String,
    /// Where the finished file goes.
    pub output: PathBuf,
    /// Which sequence; absent takes the project's only one.
    #[serde(default)]
    pub sequence: Option<SequenceId>,
    /// The frames to write; absent writes the whole sequence.
    #[serde(default)]
    pub range: Option<FrameRange>,
}

/// The parameters of [`EXPORT_PROGRESS`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressParams {
    /// The job id [`EXPORT_RENDER`] answered with.
    pub job: String,
}

/// Where an export has got to.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExportStatus {
    /// The job id, which [`EXPORT_PROGRESS`] takes.
    pub job: String,
    /// `running`, `completed`, `failed` or `cancelled`.
    pub state: String,
    /// Frames written so far.
    pub frames_done: u64,
    /// Frames the export will write in total.
    pub frames_total: u64,
    /// How far along it is, in thousandths of a percent, so nothing is
    /// reported as a float.
    pub permille: u64,
    /// Where the file is being written.
    pub output: String,
    /// The failure, as the standard JSON error, when the state is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

impl ExportStatus {
    /// A running export that has written `done` of `total` frames.
    #[must_use]
    pub fn running(
        job: impl Into<String>,
        output: impl Into<String>,
        done: u64,
        total: u64,
    ) -> Self {
        Self {
            job: job.into(),
            state: "running".to_owned(),
            frames_done: done,
            frames_total: total,
            permille: permille(done, total),
            output: output.into(),
            error: None,
        }
    }

    /// The same export, finished in `state`.
    #[must_use]
    pub fn finished(mut self, state: &str) -> Self {
        state.clone_into(&mut self.state);
        self
    }

    /// The same export, failed with `error`.
    #[must_use]
    pub fn failed(mut self, error: &SubError) -> Self {
        "failed".clone_into(&mut self.state);
        self.error = Some(error.to_json());
        self
    }
}

/// `done` of `total`, in thousandths of a percent, without floating point.
#[must_use]
pub fn permille(done: u64, total: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    done.saturating_mul(1000) / total
}

/// The exported JSON Schema of the host-backed methods.
///
/// Generated the same way [`crate::schema`] generates the engine's, and
/// committed at [`schema::COMMITTED_PATH`] so the MCP bridge can compile it in
/// without building a dispatcher.
pub mod schema {
    use super::{
        EXPORT_LIST_PRESETS, EXPORT_PROGRESS, EXPORT_RENDER, ExportParams, ExportStatus,
        FrameImage, FrameParams, MEDIA_MAKE_PROXY, MEDIA_PROBE, PLAYBACK_RENDER_FRAME_PNG,
        PresetListResult, ProbeParams, ProgressParams, ProxyParams,
    };
    use crate::dispatch::{AppliedResult, NoParams};
    use schemars::{JsonSchema, SchemaGenerator, generate::SchemaSettings};
    use serde_json::{Map, Value};

    /// The meta-schema the exported document conforms to.
    pub const META_SCHEMA: &str = "https://json-schema.org/draft/2020-12/schema";

    /// The title of the exported document.
    pub const TITLE: &str = "Subordinate host-served API";

    /// The path of the committed copy, relative to the repository root.
    pub const COMMITTED_PATH: &str = "docs/schema/host-api.json";

    /// One method's entry in the document.
    fn method<P: JsonSchema, R: JsonSchema>(
        generator: &mut SchemaGenerator,
        name: &str,
        description: &str,
    ) -> Value {
        serde_json::json!({
            "name": name,
            "kind": "query",
            "description": description,
            "params": generator.subschema_for::<P>().to_value(),
            "result": generator.subschema_for::<R>().to_value(),
        })
    }

    /// The JSON Schema of every host-served method.
    #[must_use]
    pub fn document() -> Value {
        let mut generator = SchemaSettings::draft2020_12().into_generator();
        let mut methods = vec![
            method::<NoParams, PresetListResult>(
                &mut generator,
                EXPORT_LIST_PRESETS,
                "List the export presets this build offers, with their container and codecs.",
            ),
            method::<ProgressParams, ExportStatus>(
                &mut generator,
                EXPORT_PROGRESS,
                "Read how far an export has got, and whether it finished or failed.",
            ),
            method::<ExportParams, ExportStatus>(
                &mut generator,
                EXPORT_RENDER,
                "Render a sequence to a file with an export preset, reporting the job that does \
                 it.",
            ),
            method::<ProxyParams, AppliedResult>(
                &mut generator,
                MEDIA_MAKE_PROXY,
                "Transcode a low-resolution proxy for a media item and record it on the item.",
            ),
            method::<ProbeParams, Value>(
                &mut generator,
                MEDIA_PROBE,
                "Read a media file's video and audio streams without importing it.",
            ),
            method::<FrameParams, FrameImage>(
                &mut generator,
                PLAYBACK_RENDER_FRAME_PNG,
                "Composite one frame of a sequence and return it as a PNG, so an agent can look \
                 at it.",
            ),
        ];
        methods.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
        let defs = generator.take_definitions(true);
        sorted(serde_json::json!({
            "$schema": META_SCHEMA,
            "title": TITLE,
            "description": "The methods of the Subordinate Command API that a serving process \
                            supplies rather than the engine: probing and proxying media, drawing \
                            a frame, and exporting (docs/PLAN.md §7). Generated from the Rust \
                            types; do not edit by hand.",
            "methods": methods,
            "$defs": defs,
        }))
    }

    /// The document as the text committed under `docs/schema/`.
    #[must_use]
    pub fn document_text() -> String {
        let mut text = serde_json::to_string_pretty(&document()).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Rebuilds `value` with every object's keys in sorted order, so the
    /// committed file is byte-stable whatever `serde_json` features the build
    /// turns on.
    fn sorted(value: Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut entries: Vec<(String, Value)> = object.into_iter().collect();
                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key, sorted(value)))
                        .collect::<Map<String, Value>>(),
                )
            }
            Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
            other => other,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{COMMITTED_PATH, META_SCHEMA, TITLE, document, document_text};

        #[test]
        fn every_method_is_described_in_one_sentence() {
            let document = document();
            assert_eq!(document["$schema"], META_SCHEMA);
            assert_eq!(document["title"], TITLE);
            for method in document["methods"].as_array().expect("methods") {
                let name = method["name"].as_str().expect("a name");
                let description = method["description"].as_str().expect("a description");
                assert!(description.ends_with('.'), "{name}: {description:?}");
                assert_eq!(description.matches(". ").count(), 0, "{name}");
                assert!(
                    description.chars().next().is_some_and(char::is_uppercase),
                    "{name}: {description:?}",
                );
                assert!(method["params"].is_object(), "{name} has no params schema");
            }
        }

        /// The committed copy is what the MCP bridge reads, so it must match
        /// what this build generates. `SUB_UPDATE_SCHEMA=1` rewrites it.
        #[test]
        fn committed_schema_is_up_to_date() {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(COMMITTED_PATH);
            let generated = document_text();
            if std::env::var_os("SUB_UPDATE_SCHEMA").is_some() {
                std::fs::write(&path, &generated).unwrap();
            }
            let committed = std::fs::read_to_string(&path).unwrap_or_default();
            assert_eq!(
                committed, generated,
                "the committed {COMMITTED_PATH} is stale; regenerate it with \
                 SUB_UPDATE_SCHEMA=1 cargo test -p sub-command committed_schema_is_up_to_date"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EXPORT_LIST_PRESETS, EXPORT_PROGRESS, EXPORT_RENDER, ExportParams, ExportStatus,
        FrameImage, FrameRequest, MEDIA_MAKE_PROXY, MEDIA_PROBE, PLAYBACK_RENDER_FRAME_PNG,
        Services, base64, permille, register_methods,
    };
    use crate::Dispatcher;
    use serde_json::{Value, json};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use sub_core::{SubError, SubResult, codes};
    use sub_edit::Engine;
    use sub_model::{MediaId, MediaItem, MediaPath, Project, Sequence, SequenceSettings};

    /// A host that answers every method without touching a decoder or a GPU.
    #[derive(Debug, Default)]
    struct Fake;

    impl Services for Fake {
        fn project_dir(&self) -> PathBuf {
            PathBuf::from("/projects/doc")
        }

        fn probe(&self, path: &Path) -> SubResult<Value> {
            Ok(json!({ "path": path.display().to_string(), "video": 1 }))
        }

        fn make_proxy(&self, _project: &Project, media: MediaId) -> SubResult<String> {
            Ok(format!("proxies/{media}.mp4"))
        }

        fn render_frame_png(&self, request: &FrameRequest<'_>) -> SubResult<FrameImage> {
            Ok(FrameImage {
                data: "iVBORw0KGgo=".to_owned(),
                mime_type: "image/png".to_owned(),
                width: request.width.unwrap_or(1920),
                height: 1080,
                time: request.time,
            })
        }

        fn presets(&self) -> SubResult<Vec<Value>> {
            Ok(vec![json!({ "id": "h264-mp4" })])
        }

        fn start_export(
            &self,
            _project: &Project,
            request: &ExportParams,
        ) -> SubResult<ExportStatus> {
            Ok(ExportStatus::running(
                "export-1",
                request.output.display().to_string(),
                0,
                100,
            ))
        }

        fn export_progress(&self, job: &str) -> SubResult<ExportStatus> {
            if job == "export-1" {
                Ok(ExportStatus::running("export-1", "/out.mp4", 50, 100))
            } else {
                Err(SubError::new(codes::NOT_FOUND, "no such export job"))
            }
        }
    }

    /// A dispatcher with the fake host installed, over a one-media project.
    fn fixture() -> (Engine, Dispatcher, MediaId) {
        let mut project = Project::new("Doc cut");
        let item = MediaItem::new(MediaPath::new("media/shot.mp4").expect("a relative path"));
        let media = item.id;
        project.media.push(item);
        project
            .sequences
            .push(Sequence::new("Main", SequenceSettings::default()));
        let engine = Engine::spawn(project).unwrap();
        let mut dispatcher = Dispatcher::new(engine.handle().clone());
        register_methods(&mut dispatcher, Arc::new(Fake)).expect("the host methods register");
        (engine, dispatcher, media)
    }

    #[test]
    fn every_host_method_is_served_once_installed() {
        let (engine, dispatcher, _) = fixture();
        for method in [
            MEDIA_PROBE,
            MEDIA_MAKE_PROXY,
            PLAYBACK_RENDER_FRAME_PNG,
            EXPORT_LIST_PRESETS,
            EXPORT_RENDER,
            EXPORT_PROGRESS,
        ] {
            assert!(dispatcher.contains(method), "{method} is not served");
            assert!(dispatcher.description(method).is_some(), "{method}");
        }
        engine.shutdown().unwrap();
    }

    #[test]
    fn probing_takes_a_path_or_a_media_item_but_not_both() {
        let (engine, dispatcher, media) = fixture();
        let by_path = dispatcher
            .invoke(MEDIA_PROBE, Some(json!({ "path": "/tmp/a.mov" })))
            .expect("a probe by path");
        assert_eq!(by_path["path"], "/tmp/a.mov");

        let by_media = dispatcher
            .invoke(MEDIA_PROBE, Some(json!({ "media": media })))
            .expect("a probe by media id");
        assert!(
            by_media["path"]
                .as_str()
                .is_some_and(|path| path.replace('\\', "/").ends_with("media/shot.mp4")),
            "{by_media}",
        );

        let neither = dispatcher
            .invoke(MEDIA_PROBE, Some(json!({})))
            .expect_err("nothing to probe");
        assert_eq!(neither.code.as_str(), "core.invalid_argument");
        let both = dispatcher
            .invoke(MEDIA_PROBE, Some(json!({ "path": "/a", "media": media })))
            .expect_err("two things to probe");
        assert_eq!(both.code.as_str(), "core.invalid_argument");
        engine.shutdown().unwrap();
    }

    #[test]
    fn making_a_proxy_records_it_on_the_item_as_an_undoable_edit() {
        let (engine, dispatcher, media) = fixture();
        let applied = dispatcher
            .invoke(MEDIA_MAKE_PROXY, Some(json!({ "media": media })))
            .expect("a proxy");
        assert_eq!(applied["revision"], 1);
        assert!(
            engine.handle().snapshot().media[0].proxy.path().is_some(),
            "the item carries a ready proxy",
        );
        dispatcher.invoke("edit.undo", None).expect("it undoes");
        assert!(engine.handle().snapshot().media[0].proxy.path().is_none());
        engine.shutdown().unwrap();
    }

    #[test]
    fn a_frame_comes_back_as_a_png() {
        let (engine, dispatcher, _) = fixture();
        let frame = dispatcher
            .invoke(
                PLAYBACK_RENDER_FRAME_PNG,
                Some(json!({
                    "time": { "value": 0, "rate": { "numerator": 24, "denominator": 1 } },
                    "width": 640,
                })),
            )
            .expect("a frame");
        assert_eq!(frame["mime_type"], "image/png");
        assert_eq!(frame["width"], 640);
        assert!(!frame["data"].as_str().unwrap_or_default().is_empty());
        engine.shutdown().unwrap();
    }

    #[test]
    fn an_export_reports_a_job_that_can_be_followed() {
        let (engine, dispatcher, _) = fixture();
        let started = dispatcher
            .invoke(
                EXPORT_RENDER,
                Some(json!({ "preset": "h264-mp4", "output": "/out.mp4" })),
            )
            .expect("an export starts");
        assert_eq!(started["state"], "running");
        let job = started["job"].as_str().expect("a job id").to_owned();

        let progress = dispatcher
            .invoke(EXPORT_PROGRESS, Some(json!({ "job": job })))
            .expect("progress");
        assert_eq!(progress["frames_done"], 50);
        assert_eq!(progress["permille"], 500);

        let unknown = dispatcher
            .invoke(EXPORT_PROGRESS, Some(json!({ "job": "nope" })))
            .expect_err("no such job");
        assert_eq!(unknown.code.as_str(), "core.not_found");

        let presets = dispatcher
            .invoke(EXPORT_LIST_PRESETS, None)
            .expect("presets");
        assert_eq!(presets["presets"][0]["id"], "h264-mp4");
        engine.shutdown().unwrap();
    }

    #[test]
    fn progress_is_thousandths_and_never_divides_by_zero() {
        assert_eq!(permille(0, 0), 0);
        assert_eq!(permille(1, 4), 250);
        assert_eq!(permille(100, 100), 1000);
    }

    #[test]
    fn base64_matches_the_rfc_test_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // The PNG signature, which is what a frame's data starts with.
        assert_eq!(base64(&[0x89, b'P', b'N', b'G']), "iVBORw==");
    }
}
