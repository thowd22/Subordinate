//! The project subcommands: `new`, `open`, `save` and `inspect`.
//!
//! These are the headless half of the editor: everything they report is JSON
//! on stdout, so CI and an agent read the same answers a person would.
//!
//! Two rules from docs/PLAN.md shape this module. Every mutation is a
//! [`sub_edit::Command`] applied through an [`Engine`] — `new` builds its
//! starter sequence that way rather than pushing on to `Project::sequences` —
//! and every time is a [`RationalTime`], reported as its exact unit count and
//! rate (with a timecode alongside for people), never as a float.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use sub_core::{ResultExt, SubError, SubResult, codes};
use sub_edit::Engine;
use sub_edit::commands::{AddTrack, CreateSequence};
use sub_model::{
    Bin, Clip, MediaItem, Project, Sequence, SequenceSettings, Track, TrackItem, TrackKind,
    json as project_json,
};
use sub_time::{Rational, RationalTime, TimeRange, Timecode, TimecodeRate};

/// The name a new project takes when neither `--name` nor the file name says
/// otherwise.
const DEFAULT_NAME: &str = "Untitled";

/// The sequence and tracks `new` starts a project with.
const STARTER_SEQUENCE: &str = "Main";
/// The starter video track.
const STARTER_VIDEO_TRACK: &str = "V1";
/// The starter audio track.
const STARTER_AUDIO_TRACK: &str = "A1";

/// Creates a project file at `path`.
///
/// The project gets one sequence with a video and an audio track, added
/// through the command engine so the file a CLI writes is one the editor could
/// have produced itself. An existing file is left alone unless `force` is set.
///
/// # Errors
///
/// Returns `core.invalid_argument` when `path` already exists and `force` is
/// false, whatever the engine returns if a starter command fails, and `core.io`
/// when the file cannot be written.
pub fn new(path: &Path, name: Option<&str>, force: bool) -> SubResult<Value> {
    if !force && path.exists() {
        return Err(SubError::new(
            codes::INVALID_ARGUMENT,
            "refusing to overwrite an existing file; pass --force to replace it",
        )
        .with_detail("path", path.display().to_string()));
    }
    let name = name.map_or_else(|| default_name(path), ToOwned::to_owned);

    let engine = Engine::spawn(Project::new(name))?;
    let built = starter_project(&engine);
    let stopped = engine.shutdown();
    let project = built?;
    stopped?;

    let bytes = write_project(&project, path)?;
    Ok(json!({
        "path": path.display().to_string(),
        "bytes": bytes,
        "schema_version": sub_model::SCHEMA_VERSION,
        "project": identity(&project),
        "sequences": project.sequences.iter().map(sequence_summary).collect::<Vec<_>>(),
    }))
}

/// Loads `path` and reports what came back: the schema version it was written
/// at, any migrations applied on the way in, and which media is offline.
///
/// # Errors
///
/// Returns `core.io` when the file cannot be read and whatever
/// [`project_json::from_json_with_report`] returns for a file this build
/// cannot load.
pub fn open(path: &Path) -> SubResult<Value> {
    let (project, report) = load(path)?;
    let mut report = json!({
        "path": path.display().to_string(),
        "project": identity(&project),
        "schema_version": {
            "file": report.original_version,
            "current": report.final_version,
            "migrated": report.migrated(),
            "migrations": report
                .applied
                .iter()
                .map(|migration| json!({
                    "from_version": migration.from_version,
                    "to_version": migration.to_version,
                    "description": migration.description,
                }))
                .collect::<Vec<_>>(),
        },
        "counts": counts(&project),
    });
    report["offline"] = Value::Array(offline(&project, project_dir(path)));
    Ok(report)
}

/// Writes `path` back out as deterministic project JSON, at `output` when one
/// is given and over the input otherwise.
///
/// Loading migrates an older file, so this is also how a project is brought up
/// to the current schema version without opening the editor.
///
/// # Errors
///
/// Returns `core.io` when either file cannot be read or written, and whatever
/// loading returns for a file this build cannot read.
pub fn save(path: &Path, output: Option<&Path>) -> SubResult<Value> {
    let original = read(path)?;
    let (project, load_report) = load_text(path, &original)?;
    let output = output.unwrap_or(path);
    let text = project_json::to_json(&project)?;
    let changed = output != path || text != original;
    let bytes = write_text(&text, output)?;
    Ok(json!({
        "input": path.display().to_string(),
        "output": output.display().to_string(),
        "bytes": bytes,
        "changed": changed,
        "migrated": load_report.migrated(),
        "schema_version": load_report.final_version,
        "project": identity(&project),
    }))
}

/// Reports the whole structure of the project at `path`: its media, its bins
/// and every sequence, track and item, with exact times throughout.
///
/// # Errors
///
/// Returns `core.io` when the file cannot be read, and whatever loading
/// returns for a file this build cannot read.
pub fn inspect(path: &Path) -> SubResult<Value> {
    let (project, load_report) = load(path)?;
    Ok(json!({
        "path": path.display().to_string(),
        "project": identity(&project),
        "schema_version": load_report.final_version,
        "counts": counts(&project),
        "media": project.media.iter().map(media_summary).collect::<Vec<_>>(),
        "bins": bin_summary(&project.root_bin),
        "sequences": project.sequences.iter().map(sequence_detail).collect::<Vec<_>>(),
    }))
}

/// Applies the starter commands to a freshly spawned engine and takes the
/// project back out.
fn starter_project(engine: &Engine) -> SubResult<Project> {
    let handle = engine.handle();
    let applied = handle.apply(CreateSequence::new(
        STARTER_SEQUENCE,
        SequenceSettings::default(),
    ))?;
    let sequence = applied
        .project
        .sequences
        .last()
        .ok_or_else(|| {
            SubError::new(
                codes::INTERNAL,
                "the sequence command reported success without adding a sequence",
            )
        })?
        .id;
    handle.apply(AddTrack::new(
        sequence,
        STARTER_VIDEO_TRACK,
        TrackKind::Video,
    ))?;
    let applied = handle.apply(AddTrack::new(
        sequence,
        STARTER_AUDIO_TRACK,
        TrackKind::Audio,
    ))?;
    Ok((*applied.project).clone())
}

/// The project name a file with no `--name` gets: its file stem, or
/// [`DEFAULT_NAME`] when the path has none.
fn default_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or(DEFAULT_NAME)
        .to_owned()
}

/// The directory media paths in `path`'s project are relative to.
fn project_dir(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("."))
}

/// Reads a project file as text.
fn read(path: &Path) -> SubResult<String> {
    fs::read_to_string(path)
        .sub_context_with(codes::IO, || "the project file could not be read")
        .map_err(|error| error.with_detail("path", path.display().to_string()))
}

/// Reads and loads a project file, migrating it if it is an older one.
pub(crate) fn load(path: &Path) -> SubResult<(Project, sub_model::LoadReport)> {
    let text = read(path)?;
    load_text(path, &text)
}

/// Loads already-read text, naming the file in whatever the load rejects.
fn load_text(path: &Path, text: &str) -> SubResult<(Project, sub_model::LoadReport)> {
    project_json::from_json_with_report(text)
        .map_err(|error| error.with_detail("path", path.display().to_string()))
}

/// Serialises `project` and writes it, returning how many bytes went out.
fn write_project(project: &Project, path: &Path) -> SubResult<usize> {
    write_text(&project_json::to_json(project)?, path)
}

/// Writes project text, returning its length in bytes.
fn write_text(text: &str, path: &Path) -> SubResult<usize> {
    fs::write(path, text)
        .sub_context_with(codes::IO, || "the project file could not be written")
        .map_err(|error| error.with_detail("path", path.display().to_string()))?;
    Ok(text.len())
}

/// The identity every subcommand repeats back, so an agent can tell two
/// answers apart.
fn identity(project: &Project) -> Value {
    json!({ "id": project.id.to_string(), "name": project.name })
}

/// How much of everything the project holds.
fn counts(project: &Project) -> Value {
    json!({
        "media": project.media.len(),
        "sequences": project.sequences.len(),
        "bins": project.root_bin.iter().count(),
    })
}

/// The media whose files are not where the project says they are.
///
/// The check runs on a copy: an inspection never edits, and marking media
/// offline in the saved file would be an edit.
fn offline(project: &Project, directory: &Path) -> Vec<Value> {
    let mut probe = project.clone();
    probe.refresh_offline(directory);
    probe
        .offline_media()
        .map(|item| {
            json!({
                "id": item.id.to_string(),
                "name": item.name,
                "path": item.path.as_str(),
                "resolved": item.absolute_path(directory).display().to_string(),
            })
        })
        .collect()
}

/// One line about a sequence, as `new` reports what it made.
fn sequence_summary(sequence: &Sequence) -> Value {
    json!({
        "id": sequence.id.to_string(),
        "name": sequence.name,
        "frame_rate": rate(sequence.settings.frame_rate),
        "tracks": sequence.tracks.iter().map(|track| track.name.clone()).collect::<Vec<_>>(),
    })
}

/// Everything `inspect` says about one sequence.
fn sequence_detail(sequence: &Sequence) -> Value {
    let timebase = sequence.settings.frame_rate;
    let duration = sequence
        .tracks
        .iter()
        .map(|track| track.duration(timebase))
        .max()
        .unwrap_or_else(|| RationalTime::zero(timebase));
    json!({
        "id": sequence.id.to_string(),
        "name": sequence.name,
        "settings": settings(&sequence.settings),
        "duration": time(duration, timebase),
        "tracks": sequence
            .tracks
            .iter()
            .map(|track| track_detail(track, timebase))
            .collect::<Vec<_>>(),
        "markers": sequence
            .markers
            .iter()
            .map(|marker| json!({
                "id": marker.id.to_string(),
                "name": marker.name,
                "note": marker.note,
                "range": range(marker.marked_range, timebase),
            }))
            .collect::<Vec<_>>(),
    })
}

/// The sequence settings, with the frame rate as an exact fraction.
fn settings(settings: &SequenceSettings) -> Value {
    json!({
        "resolution": {
            "width": settings.resolution.width(),
            "height": settings.resolution.height(),
        },
        "frame_rate": rate(settings.frame_rate),
        "sample_rate": settings.sample_rate,
    })
}

/// Everything `inspect` says about one track, including where each item sits
/// in sequence time.
fn track_detail(track: &Track, timebase: Rational) -> Value {
    json!({
        "id": track.id.to_string(),
        "name": track.name,
        "kind": match track.kind {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
        },
        "muted": track.muted,
        "locked": track.locked,
        "duration": time(track.duration(timebase), timebase),
        "items": track
            .placements(timebase)
            .map(|(item, placement)| item_detail(item, placement, timebase))
            .collect::<Vec<_>>(),
    })
}

/// One item on a track, with the span it occupies in sequence time.
fn item_detail(item: &TrackItem, placement: TimeRange, timebase: Rational) -> Value {
    let mut object = match item {
        TrackItem::Clip(clip) => clip_detail(clip, timebase),
        TrackItem::Gap(gap) => json!({
            "kind": "gap",
            "duration": time(gap.duration, timebase),
        }),
        TrackItem::Transition(transition) => json!({
            "kind": "transition",
            "duration": time(transition.duration(), timebase),
        }),
    };
    object["timeline"] = range(placement, timebase);
    object
}

/// One clip: its identity, its source span and its parameters.
fn clip_detail(clip: &Clip, timebase: Rational) -> Value {
    json!({
        "kind": "clip",
        "id": clip.id.to_string(),
        "name": clip.name,
        "media": clip.media.to_string(),
        "source": range(clip.source_range, timebase),
        "fade_in": time(clip.fade_in, timebase),
        "fade_out": time(clip.fade_out, timebase),
        "markers": clip.markers.len(),
    })
}

/// One media item, with whatever the probe knows about its streams.
fn media_summary(item: &MediaItem) -> Value {
    let mut object = json!({
        "id": item.id.to_string(),
        "name": item.name,
        "path": item.path.as_str(),
        "offline": item.offline,
        "hash": item.hash.as_ref().map(ToString::to_string),
    });
    if let Some(info) = &item.info {
        let mut streams = Map::new();
        if let Some(duration) = info.duration {
            streams.insert("duration".to_owned(), time(duration, duration.rate()));
        }
        streams.insert(
            "video".to_owned(),
            Value::Array(
                info.video
                    .iter()
                    .map(|stream| {
                        json!({
                            "width": stream.width,
                            "height": stream.height,
                            "frame_rate": rate(stream.frame_rate),
                        })
                    })
                    .collect(),
            ),
        );
        streams.insert(
            "audio".to_owned(),
            Value::Array(
                info.audio
                    .iter()
                    .map(|stream| {
                        json!({
                            "channels": stream.channels,
                            "sample_rate": stream.sample_rate,
                        })
                    })
                    .collect(),
            ),
        );
        object["streams"] = Value::Object(streams);
    }
    object
}

/// A bin and everything under it.
fn bin_summary(bin: &Bin) -> Value {
    json!({
        "id": bin.id.to_string(),
        "name": bin.name,
        "media": bin.media.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "children": bin.children.iter().map(bin_summary).collect::<Vec<_>>(),
    })
}

/// A rate as the exact fraction it is.
fn rate(value: Rational) -> Value {
    json!({
        "numerator": value.numerator(),
        "denominator": value.denominator(),
        "text": value.to_string(),
    })
}

/// A time as its exact unit count and rate, its exact length in seconds as a
/// fraction, and the timecode a person reads it as at `timebase`.
///
/// No float appears anywhere in this, which is the point: `1001/30000` of a
/// second is not a `f64` and never becomes one on the way to a report.
fn time(value: RationalTime, timebase: Rational) -> Value {
    let (numerator, denominator) = value.as_seconds_fraction();
    let mut object = json!({
        "value": value.value(),
        "rate": rate(value.rate()),
        "seconds": { "numerator": numerator.to_string(), "denominator": denominator.to_string() },
    });
    if let Some(code) = timecode(value, timebase) {
        object["timecode"] = Value::String(code);
    }
    object
}

/// A span as its start, duration and exclusive end.
fn range(value: TimeRange, timebase: Rational) -> Value {
    json!({
        "start": time(value.start(), timebase),
        "duration": time(value.duration(), timebase),
        "end_exclusive": time(value.end_exclusive(), timebase),
    })
}

/// `HH:MM:SS:FF` at `timebase`, or `None` when the rate has no timecode form
/// or the time does not land on a frame of it.
fn timecode(value: RationalTime, timebase: Rational) -> Option<String> {
    let rate = TimecodeRate::new(timebase, TimecodeRate::rate_drops_frames(timebase)).ok()?;
    Timecode::from_rational_time(value, rate)
        .ok()
        .map(|code| code.to_string())
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_NAME, default_name, rate, time, timecode};
    use std::path::Path;
    use sub_time::{Rational, RationalTime};

    #[test]
    fn a_new_project_is_named_after_its_file() {
        assert_eq!(default_name(Path::new("/tmp/Doc cut.sub")), "Doc cut");
        assert_eq!(default_name(Path::new("cut.sub")), "cut");
        assert_eq!(default_name(Path::new("/")), DEFAULT_NAME);
    }

    #[test]
    fn a_rate_is_reported_as_the_fraction_it_is() {
        let ntsc = rate(Rational::FPS_29_97);
        assert_eq!(ntsc["numerator"], 30000);
        assert_eq!(ntsc["denominator"], 1001);
    }

    #[test]
    fn a_time_keeps_its_units_and_carries_exact_seconds() {
        let value = time(
            RationalTime::from_frames(48, Rational::FPS_24),
            Rational::FPS_24,
        );
        assert_eq!(value["value"], 48);
        assert_eq!(value["rate"]["numerator"], 24);
        assert_eq!(value["seconds"]["numerator"], "2");
        assert_eq!(value["seconds"]["denominator"], "1");
        assert_eq!(value["timecode"], "00:00:02:00");
        assert!(
            !value["seconds"]["numerator"].is_f64(),
            "seconds must never be a float"
        );
    }

    #[test]
    fn a_drop_frame_rate_reports_drop_frame_timecode() {
        let at = RationalTime::from_frames(1800, Rational::FPS_29_97);
        assert_eq!(
            timecode(at, Rational::FPS_29_97).as_deref(),
            Some("00;01;00;02")
        );
    }
}
