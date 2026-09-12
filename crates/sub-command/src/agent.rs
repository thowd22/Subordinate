//! The agent-facing tool families the MCP bridge publishes (docs/PLAN.md §7).
//!
//! The Command API is organised by the entity an edit touches — `clip.*`,
//! `track.*`, `marker.*` — because that is how the command set is built. An
//! agent reads a tool list task-first instead, so the plan names five families
//! and this module is the part of them the engine alone can serve:
//!
//! - `project.*` — [`PROJECT_NEW`], [`PROJECT_OPEN`], [`PROJECT_SAVE`],
//!   [`PROJECT_LIST_SEQUENCES`] and [`PROJECT_SETTINGS`].
//! - `media.*` — [`MEDIA_LIST`]. Importing and relinking are already commands.
//! - `timeline.*` — the clip, track and marker edits under the names the plan
//!   uses, plus [`TIMELINE_GET_STATE`], which hands back one sequence in the
//!   OTIO-shaped JSON the project file stores.
//! - `playback.*` — [`PLAYBACK_PLAY`], [`PLAYBACK_PAUSE`], [`PLAYBACK_SEEK`]
//!   and [`PLAYBACK_STATUS`] over the engine's transport.
//!
//! The rest of `media.*`, `playback.render_frame_png` and the whole of
//! `export.*` need decoders, a GPU and an encoder, which the engine does not
//! own; they live in [`crate::host`] and are installed by whoever is serving.
//!
//! Two rules shape what is here.
//!
//! - **Nothing is a second implementation.** Every `timeline.*` mutation is an
//!   alias for a registered command kind, applied through the same envelope,
//!   the same registry and the same history, and carrying the command's own
//!   parameter schema into the exported document. Renaming a family member is
//!   therefore never a behaviour change (decision-7).
//! - **Opening and starting a document are edits.** [`PROJECT_NEW`] and
//!   [`PROJECT_OPEN`] apply [`ReplaceProject`], so an agent that opened the
//!   wrong file undoes it like any other mistake.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sub_core::{SubError, SubResult, codes};
use sub_edit::commands::ReplaceProject;
use sub_edit::playback::sequence_duration;
use sub_edit::{EngineHandle, PlaybackOp, PlaybackStatus, ShuttleSpeed};
use sub_model::{MediaItem, Project, Sequence, SequenceId, SequenceSettings, json};
use sub_time::{RationalTime, TimeRange};

use crate::dispatch::{AppliedResult, Dispatcher, NoParams, to_value, typed};

/// Start a new, empty project.
pub const PROJECT_NEW: &str = "project.new";
/// Open a project file.
pub const PROJECT_OPEN: &str = "project.open";
/// Write the open project to a file.
pub const PROJECT_SAVE: &str = "project.save";
/// List the project's sequences.
pub const PROJECT_LIST_SEQUENCES: &str = "project.list_sequences";
/// Read the project's settings.
pub const PROJECT_SETTINGS: &str = "project.settings";
/// List the media the project references.
pub const MEDIA_LIST: &str = "media.list";
/// Read one sequence whole.
pub const TIMELINE_GET_STATE: &str = "timeline.get_state";
/// Start playback.
pub const PLAYBACK_PLAY: &str = "playback.play";
/// Stop playback.
pub const PLAYBACK_PAUSE: &str = "playback.pause";
/// Move the playhead.
pub const PLAYBACK_SEEK: &str = "playback.seek";
/// Read the transport state.
pub const PLAYBACK_STATUS: &str = "playback.status";

/// Every `timeline.*` mutation, as the name it is published under and the
/// command kind it applies.
///
/// The order is the order of docs/PLAN.md §7: the clip edits, then tracks,
/// then markers.
pub const TIMELINE_ALIASES: &[(&str, &str, &str)] = &[
    (
        "timeline.add_clip",
        "Add a clip to a track of a sequence at a given start time.",
        "clip.add",
    ),
    (
        "timeline.move_clip",
        "Move a clip along its track, or to another track of the same sequence.",
        "clip.move",
    ),
    (
        "timeline.trim_clip_in",
        "Move a clip's in point, keeping its out point where it is.",
        "clip.trim_in",
    ),
    (
        "timeline.trim_clip_out",
        "Move a clip's out point, keeping its in point where it is.",
        "clip.trim_out",
    ),
    (
        "timeline.split_clip",
        "Split a clip in two at a time inside it.",
        "clip.split",
    ),
    (
        "timeline.delete_clip",
        "Remove a clip, leaving a gap where it was.",
        "clip.remove",
    ),
    (
        "timeline.ripple_delete_clip",
        "Remove a clip and close the gap by pulling everything after it back.",
        "clip.ripple_delete",
    ),
    (
        "timeline.add_track",
        "Add a track to a sequence.",
        "track.add",
    ),
    (
        "timeline.remove_track",
        "Remove a track from a sequence, with everything on it.",
        "track.remove",
    ),
    (
        "timeline.add_marker",
        "Add a marker to a sequence or to a clip.",
        "marker.add",
    ),
    (
        "timeline.remove_marker",
        "Remove a marker from a sequence or from a clip.",
        "marker.remove",
    ),
];

/// Puts the engine-served families on `dispatcher`.
///
/// Called by [`Dispatcher::with_registry`], so every dispatcher in the build
/// serves the same families and the exported schema lists them all.
pub(crate) fn install(dispatcher: &mut Dispatcher) {
    install_project(dispatcher);
    install_media(dispatcher);
    install_timeline(dispatcher);
    install_playback(dispatcher);
}

/// The `project.*` family.
fn install_project(dispatcher: &mut Dispatcher) {
    let host = dispatcher.host_services.clone();
    dispatcher.add::<NewProjectParams, AppliedResult, _>(
        PROJECT_NEW,
        "Replace the open project with a new, empty one, undoably.",
        move |engine, params| {
            let params: NewProjectParams = typed(params)?;
            let project = Project::new(params.name);
            let result = apply_project(engine, project.clone())?;
            if let Some(host) = host
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                host.project_file_changed(&project, None);
            }
            Ok(result)
        },
    );
    let host = dispatcher.host_services.clone();
    dispatcher.add::<OpenProjectParams, AppliedResult, _>(
        PROJECT_OPEN,
        "Open a project file, replacing the open project undoably.",
        move |engine, params| {
            let params: OpenProjectParams = typed(params)?;
            let project = read_project(&params.path)?;
            let result = apply_project(engine, project.clone())?;
            if let Some(host) = host
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                host.project_file_changed(&project, Some(&params.path));
            }
            Ok(result)
        },
    );
    let host = dispatcher.host_services.clone();
    dispatcher.add::<SaveProjectParams, SaveResult, _>(
        PROJECT_SAVE,
        "Write the open project to a file as JSON.",
        move |engine, params| {
            let params: SaveProjectParams = typed(params)?;
            let revision = engine.revision();
            let project = engine.snapshot();
            let text = json::to_json(&project)?;
            write_project(&params.path, &text)?;
            if let Some(host) = host
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                host.project_file_changed(&project, Some(&params.path));
            }
            to_value(&SaveResult {
                path: params.path.display().to_string(),
                bytes: text.len() as u64,
                revision,
            })
        },
    );
    dispatcher.add::<NoParams, SequenceListResult, _>(
        PROJECT_LIST_SEQUENCES,
        "List the project's sequences, with their settings and their length.",
        |engine, params| {
            typed::<NoParams>(params)?;
            let project = engine.snapshot();
            to_value(&SequenceListResult {
                revision: engine.revision(),
                sequences: project.sequences.iter().map(SequenceSummary::of).collect(),
            })
        },
    );
    dispatcher.add::<NoParams, SettingsResult, _>(
        PROJECT_SETTINGS,
        "Read the open project's name, identity and per-sequence settings.",
        |engine, params| {
            typed::<NoParams>(params)?;
            let project = engine.snapshot();
            to_value(&SettingsResult {
                revision: engine.revision(),
                id: project.id.to_string(),
                name: project.name.clone(),
                media_count: project.media.len(),
                sequences: project.sequences.iter().map(SequenceSummary::of).collect(),
            })
        },
    );
}

/// The engine-served part of the `media.*` family.
fn install_media(dispatcher: &mut Dispatcher) {
    dispatcher.add::<NoParams, MediaListResult, _>(
        MEDIA_LIST,
        "List every media item the project references, in project order.",
        |engine, params| {
            typed::<NoParams>(params)?;
            let project = engine.snapshot();
            to_value(&MediaListResult {
                revision: engine.revision(),
                media: project.media.clone(),
            })
        },
    );
}

/// The `timeline.*` family: the aliases, then the state query.
fn install_timeline(dispatcher: &mut Dispatcher) {
    for (name, description, kind) in TIMELINE_ALIASES {
        dispatcher.alias(name, description, kind);
    }
    dispatcher.add::<SequenceParams, TimelineStateResult, _>(
        TIMELINE_GET_STATE,
        "Read one sequence whole, as the OTIO-shaped JSON the project file stores.",
        |engine, params| {
            let params: SequenceParams = typed(params)?;
            let project = engine.snapshot();
            let sequence = pick_sequence(&project, params.sequence)?;
            to_value(&TimelineStateResult {
                revision: engine.revision(),
                duration: sequence_duration(sequence),
                sequence: sequence.clone(),
            })
        },
    );
}

/// The engine-served part of the `playback.*` family.
fn install_playback(dispatcher: &mut Dispatcher) {
    dispatcher.add::<PlayParams, TransportResult, _>(
        PLAYBACK_PLAY,
        "Start playback, forwards or backwards, at a shuttle speed.",
        |engine, params| {
            let params: PlayParams = typed(params)?;
            if let Some(sequence) = params.sequence {
                engine.follow_sequence(sequence)?;
            }
            let status = engine.playback(PlaybackOp::SetSpeed(params.speed))?;
            to_value(&TransportResult::of(&status))
        },
    );
    dispatcher.add::<NoParams, TransportResult, _>(
        PLAYBACK_PAUSE,
        "Stop playback, leaving the playhead where it stands.",
        |engine, params| {
            typed::<NoParams>(params)?;
            to_value(&TransportResult::of(&engine.pause_playback()?))
        },
    );
    dispatcher.add::<SeekParams, TransportResult, _>(
        PLAYBACK_SEEK,
        "Move the playhead to an exact time, without stopping playback.",
        |engine, params| {
            let params: SeekParams = typed(params)?;
            if let Some(sequence) = params.sequence {
                engine.follow_sequence(sequence)?;
            }
            to_value(&TransportResult::of(&engine.seek(params.position)?))
        },
    );
    dispatcher.add::<NoParams, TransportResult, _>(
        PLAYBACK_STATUS,
        "Read where the playhead is and whether playback is running.",
        |engine, params| {
            typed::<NoParams>(params)?;
            to_value(&TransportResult::of(&engine.playback_status()?))
        },
    );
}

/// Applies a whole-project swap through the history.
fn apply_project(engine: &EngineHandle, project: Project) -> SubResult<Value> {
    let applied = engine.apply(ReplaceProject::new(project))?;
    to_value(&AppliedResult::from(&applied))
}

/// Reads a project file, reporting the path with whatever went wrong.
fn read_project(path: &Path) -> SubResult<Project> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        SubError::new(codes::IO, "the project file could not be read")
            .with_detail("path", path.display().to_string())
            .with_cause(&error)
    })?;
    json::from_json(&text).map_err(|error| error.with_detail("path", path.display().to_string()))
}

/// Writes a project file, reporting the path with whatever went wrong.
fn write_project(path: &Path, text: &str) -> SubResult<()> {
    std::fs::write(path, text).map_err(|error| {
        SubError::new(codes::IO, "the project file could not be written")
            .with_detail("path", path.display().to_string())
            .with_cause(&error)
    })
}

/// The sequence a call names, or the project's only one.
///
/// # Errors
///
/// `core.not_found` when the project holds no such sequence, and
/// `core.invalid_argument` when no sequence was named and the project does not
/// have exactly one.
pub fn pick_sequence(project: &Project, sequence: Option<SequenceId>) -> SubResult<&Sequence> {
    match sequence {
        Some(id) => project.sequence(id).ok_or_else(|| {
            SubError::new(codes::NOT_FOUND, "no such sequence in the project")
                .with_detail("sequence", id.to_string())
        }),
        None => match project.sequences.as_slice() {
            [only] => Ok(only),
            [] => Err(SubError::new(
                codes::INVALID_ARGUMENT,
                "the project has no sequences",
            )),
            many => Err(SubError::new(
                codes::INVALID_ARGUMENT,
                "the project has more than one sequence, so one must be named",
            )
            .with_detail("sequences", many.len())),
        },
    }
}

/// The parameters of [`PROJECT_NEW`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewProjectParams {
    /// The name the new project takes.
    pub name: String,
}

/// The parameters of [`PROJECT_OPEN`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenProjectParams {
    /// The project file to read.
    pub path: PathBuf,
}

/// The parameters of [`PROJECT_SAVE`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveProjectParams {
    /// Where the project file is written.
    pub path: PathBuf,
}

/// The parameters of every method that takes an optional sequence.
#[derive(Debug, Clone, Default, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequenceParams {
    /// Which sequence; absent takes the project's only one.
    #[serde(default)]
    pub sequence: Option<SequenceId>,
}

/// The parameters of [`PLAYBACK_PLAY`].
#[derive(Debug, Clone, Default, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayParams {
    /// How fast, and in which direction; the default is forward at 1x.
    #[serde(default = "forward")]
    pub speed: ShuttleSpeed,
    /// The sequence to play; absent keeps the transport on the one it is on.
    #[serde(default)]
    pub sequence: Option<SequenceId>,
}

/// The speed a `playback.play` with no speed runs at.
fn forward() -> ShuttleSpeed {
    ShuttleSpeed::Forward1x
}

/// The parameters of [`PLAYBACK_SEEK`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeekParams {
    /// Where the playhead goes, as an exact rational time.
    pub position: RationalTime,
    /// The sequence to seek in; absent keeps the transport on the one it is on.
    #[serde(default)]
    pub sequence: Option<SequenceId>,
}

/// The result of [`PROJECT_SAVE`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct SaveResult {
    /// Where the file was written.
    pub path: String,
    /// How many bytes it holds.
    pub bytes: u64,
    /// The revision that was written.
    pub revision: u64,
}

/// One sequence, as the listing describes it.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct SequenceSummary {
    /// The sequence's stable identifier.
    pub id: SequenceId,
    /// Its display name.
    pub name: String,
    /// Its canvas, timebase, audio rate and colour tags.
    pub settings: SequenceSettings,
    /// How many tracks it holds.
    pub tracks: usize,
    /// How far its last item reaches.
    pub duration: RationalTime,
}

impl SequenceSummary {
    /// Describes `sequence`.
    #[must_use]
    pub fn of(sequence: &Sequence) -> Self {
        Self {
            id: sequence.id,
            name: sequence.name.clone(),
            settings: sequence.settings,
            tracks: sequence.tracks.len(),
            duration: sequence_duration(sequence),
        }
    }
}

/// The result of [`PROJECT_LIST_SEQUENCES`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct SequenceListResult {
    /// The revision the project was read at.
    pub revision: u64,
    /// The sequences, in tab order.
    pub sequences: Vec<SequenceSummary>,
}

/// The result of [`PROJECT_SETTINGS`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct SettingsResult {
    /// The revision the project was read at.
    pub revision: u64,
    /// The project's stable identifier.
    pub id: String,
    /// Its display name.
    pub name: String,
    /// How many media items it references.
    pub media_count: usize,
    /// The settings of each sequence, which is where canvas and timebase live.
    pub sequences: Vec<SequenceSummary>,
}

/// The result of [`MEDIA_LIST`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct MediaListResult {
    /// The revision the project was read at.
    pub revision: u64,
    /// Every media item, in project order.
    pub media: Vec<MediaItem>,
}

/// The result of [`TIMELINE_GET_STATE`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct TimelineStateResult {
    /// The revision the sequence was read at.
    pub revision: u64,
    /// How far the sequence's last item reaches.
    pub duration: RationalTime,
    /// The sequence itself: tracks, clips, transitions and markers.
    pub sequence: Sequence,
}

/// The result of every `playback.*` method the engine serves.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct TransportResult {
    /// Where the playhead is.
    pub position: RationalTime,
    /// How long the sequence being played is.
    pub duration: RationalTime,
    /// The shuttle speed.
    pub speed: ShuttleSpeed,
    /// Whether playback is running.
    pub playing: bool,
    /// The loop range in force, if any.
    pub loop_range: Option<TimeRange>,
    /// Frames dropped in this run of playback.
    pub dropped_frames: u64,
}

impl TransportResult {
    /// Describes `status`.
    #[must_use]
    pub fn of(status: &PlaybackStatus) -> Self {
        Self {
            position: status.position,
            duration: status.duration,
            speed: status.speed,
            playing: status.playing,
            loop_range: status.loop_range,
            dropped_frames: status.dropped_frames,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MEDIA_LIST, PLAYBACK_PAUSE, PLAYBACK_PLAY, PLAYBACK_SEEK, PLAYBACK_STATUS,
        PROJECT_LIST_SEQUENCES, PROJECT_NEW, PROJECT_OPEN, PROJECT_SAVE, PROJECT_SETTINGS,
        TIMELINE_ALIASES, TIMELINE_GET_STATE,
    };
    use crate::Dispatcher;
    use serde_json::json;
    use sub_edit::Engine;
    use sub_model::{Project, Sequence, SequenceSettings};

    /// A dispatcher over a project holding one sequence with one track.
    fn fixture() -> (Engine, Dispatcher) {
        let mut project = Project::new("Doc cut");
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence
            .tracks
            .push(sub_model::Track::new("V1", sub_model::TrackKind::Video));
        project.sequences.push(sequence);
        let engine = Engine::spawn(project).unwrap();
        let dispatcher = Dispatcher::new(engine.handle().clone());
        (engine, dispatcher)
    }

    #[test]
    fn every_family_member_is_served() {
        let (engine, dispatcher) = fixture();
        for method in [
            PROJECT_NEW,
            PROJECT_OPEN,
            PROJECT_SAVE,
            PROJECT_LIST_SEQUENCES,
            PROJECT_SETTINGS,
            MEDIA_LIST,
            TIMELINE_GET_STATE,
            PLAYBACK_PLAY,
            PLAYBACK_PAUSE,
            PLAYBACK_SEEK,
            PLAYBACK_STATUS,
        ] {
            assert!(dispatcher.contains(method), "{method} is not served");
            assert!(
                dispatcher.description(method).is_some(),
                "{method} has no description",
            );
        }
        for (name, description, kind) in TIMELINE_ALIASES {
            assert!(dispatcher.contains(name), "{name} is not served");
            assert!(dispatcher.contains(kind), "{kind} is not a command");
            assert_eq!(dispatcher.description(name), Some(*description));
        }
        engine.shutdown().unwrap();
    }

    #[test]
    fn a_timeline_alias_applies_the_command_it_names() {
        let (engine, dispatcher) = fixture();
        let sequence = engine.handle().snapshot().sequences[0].id;
        let added = dispatcher
            .invoke(
                "timeline.add_track",
                Some(json!({ "sequence": sequence, "kind": "video", "name": "V2" })),
            )
            .expect("the alias applies track.add");
        assert_eq!(added["revision"], 1);
        assert_eq!(engine.handle().snapshot().sequences[0].tracks.len(), 2);

        // It is one history step of the aliased command, so it undoes.
        dispatcher
            .invoke("edit.undo", None)
            .expect("the step undoes");
        assert_eq!(engine.handle().snapshot().sequences[0].tracks.len(), 1);
        engine.shutdown().unwrap();
    }

    #[test]
    fn an_alias_is_reported_as_a_command_and_refuses_bad_parameters() {
        let (engine, dispatcher) = fixture();
        let kinds: Vec<String> = dispatcher
            .methods()
            .into_iter()
            .filter(|method| method.name == "timeline.add_track")
            .map(|method| method.kind)
            .collect();
        assert_eq!(kinds, ["command"]);

        let error = dispatcher
            .invoke("timeline.add_track", Some(json!([])))
            .expect_err("an array is not a command's parameters");
        assert_eq!(error.code.as_str(), "command.invalid_params");
        engine.shutdown().unwrap();
    }

    #[test]
    fn a_new_project_replaces_the_open_one_undoably() {
        let (engine, dispatcher) = fixture();
        dispatcher
            .invoke(PROJECT_NEW, Some(json!({ "name": "Trailer" })))
            .expect("a new project");
        assert_eq!(engine.handle().snapshot().name, "Trailer");
        assert!(engine.handle().snapshot().sequences.is_empty());

        dispatcher
            .invoke("edit.undo", None)
            .expect("the swap undoes");
        assert_eq!(engine.handle().snapshot().name, "Doc cut");
        assert_eq!(engine.handle().snapshot().sequences.len(), 1);
        engine.shutdown().unwrap();
    }

    #[test]
    fn a_project_saves_and_opens_again() {
        let (engine, dispatcher) = fixture();
        let dir = std::env::temp_dir().join(format!("sub-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        let path = dir.join("doc.sub");

        let saved = dispatcher
            .invoke(PROJECT_SAVE, Some(json!({ "path": path })))
            .expect("the project saves");
        assert!(saved["bytes"].as_u64().unwrap_or(0) > 0);

        dispatcher
            .invoke(PROJECT_NEW, Some(json!({ "name": "Scratch" })))
            .expect("a new project");
        dispatcher
            .invoke(PROJECT_OPEN, Some(json!({ "path": path })))
            .expect("the saved project opens");
        assert_eq!(engine.handle().snapshot().name, "Doc cut");

        let missing = dispatcher
            .invoke(PROJECT_OPEN, Some(json!({ "path": dir.join("gone.sub") })))
            .expect_err("no such file");
        assert_eq!(missing.code.as_str(), "core.io");
        assert!(missing.to_json().to_string().contains("gone.sub"));

        std::fs::remove_dir_all(&dir).ok();
        engine.shutdown().unwrap();
    }

    #[test]
    fn the_listings_describe_the_project() {
        let (engine, dispatcher) = fixture();
        let sequences = dispatcher
            .invoke(PROJECT_LIST_SEQUENCES, None)
            .expect("a listing");
        assert_eq!(sequences["sequences"].as_array().map(Vec::len), Some(1));
        assert_eq!(sequences["sequences"][0]["name"], "Main");
        assert_eq!(sequences["sequences"][0]["tracks"], 1);

        let settings = dispatcher.invoke(PROJECT_SETTINGS, None).expect("settings");
        assert_eq!(settings["name"], "Doc cut");
        assert_eq!(settings["media_count"], 0);

        let media = dispatcher
            .invoke(MEDIA_LIST, None)
            .expect("a media listing");
        assert_eq!(media["media"].as_array().map(Vec::len), Some(0));
        engine.shutdown().unwrap();
    }

    #[test]
    fn the_timeline_state_is_the_sequence_itself() {
        let (engine, dispatcher) = fixture();
        let state = dispatcher
            .invoke(TIMELINE_GET_STATE, None)
            .expect("the only sequence");
        assert_eq!(state["sequence"]["name"], "Main");
        assert_eq!(
            state["sequence"]["tracks"].as_array().map(Vec::len),
            Some(1)
        );
        assert!(state["duration"].is_object(), "an exact rational time");

        let unknown = dispatcher
            .invoke(
                TIMELINE_GET_STATE,
                Some(json!({ "sequence": sub_model::SequenceId::new() })),
            )
            .expect_err("no such sequence");
        assert_eq!(unknown.code.as_str(), "core.not_found");
        engine.shutdown().unwrap();
    }

    #[test]
    fn the_transport_plays_seeks_and_pauses() {
        let (engine, dispatcher) = fixture();
        let sequence = engine.handle().snapshot().sequences[0].id;
        let playing = dispatcher
            .invoke(
                PLAYBACK_PLAY,
                Some(json!({ "speed": "forward2x", "sequence": sequence })),
            )
            .expect("playback starts");
        assert_eq!(playing["playing"], true);
        assert_eq!(playing["speed"], "forward2x");

        let sought = dispatcher
            .invoke(
                PLAYBACK_SEEK,
                Some(json!({ "position": { "value": 5, "rate": { "numerator": 24, "denominator": 1 } } })),
            )
            .expect("the playhead moves");
        // The fixture sequence is empty, so the transport clamps to its end.
        assert_eq!(sought["position"], sought["duration"]);
        assert_eq!(sought["position"]["value"], 0);

        let paused = dispatcher.invoke(PLAYBACK_PAUSE, None).expect("it pauses");
        assert_eq!(paused["playing"], false);
        let status = dispatcher.invoke(PLAYBACK_STATUS, None).expect("a status");
        assert_eq!(status["playing"], false);
        engine.shutdown().unwrap();
    }
}
