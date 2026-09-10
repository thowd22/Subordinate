//! Turning an OTIO timeline into the description the host applies.
//!
//! An importer never edits the project: it describes what it found and the
//! host applies that description through the Command API, so an import is an
//! ordinary undoable step and every identifier stays the host's to mint. This
//! module produces that description in plain Rust types; the component crate
//! maps them onto the `importer-api` records.
//!
//! Reading someone else's OTIO means deciding what to do with what the MVP
//! model has no place for, and the rule throughout is *keep the cut, say what
//! was dropped*:
//!
//! - a clip with no `ExternalReference` — a `GeneratorReference` (Kdenlive
//!   writes one for a colour clip), a `MissingReference`, or nothing at all —
//!   becomes a gap of the same length, so everything after it stays where the
//!   author put it;
//! - a track that is neither `Video` nor `Audio` (Kdenlive exports a
//!   `Subtitle` track) is skipped;
//! - `effects` are ignored, in both directions;
//! - markers on a track, rather than on the timeline or a clip, are ignored:
//!   the model has no track markers.
//!
//! Every one of those produces a line in [`ImportPlan::notes`], which the
//! component logs, so an import is never quietly lossy.

use serde_json::Value;

use crate::error::{Error, Result, codes, invalid_document};
use crate::project::{Resolution, Settings};
use crate::schema::{self, Item, Marker, Timeline, Track};
use crate::time::{Rate, Span, Time};

/// Everything one OTIO document asks the host to create.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportPlan {
    /// The media files the sequence refers to, in first-use order.
    pub media: Vec<ImportedMedia>,
    /// The sequence itself.
    pub sequence: ImportedSequence,
    /// What was dropped or guessed, one line each, for the log.
    pub notes: Vec<String>,
}

/// One media file the import wants filed in the bin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedMedia {
    /// Project-relative path with `/` separators; never absolute, never `..`.
    pub path: String,
    /// Display name for the bin.
    pub name: String,
}

/// The sequence an import produces.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedSequence {
    /// Display name for the sequence tab.
    pub name: String,
    /// The exact timebase.
    pub frame_rate: Rate,
    /// The canvas size.
    pub resolution: Resolution,
    /// The audio sample rate in hertz.
    pub sample_rate: u32,
    /// The lanes, in document order.
    pub tracks: Vec<ImportedTrack>,
    /// Markers on the sequence itself, in sequence time.
    pub markers: Vec<ImportedMarker>,
}

/// One lane of an imported sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTrack {
    /// Display name for the track header.
    pub name: String,
    /// Whether the lane carries picture or sound.
    pub kind: TrackKind,
    /// The items, in playback order.
    pub items: Vec<ImportedItem>,
}

/// What an imported lane carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    /// Picture.
    Video,
    /// Sound.
    Audio,
}

/// One entry in an imported lane.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportedItem {
    /// A piece of media.
    Clip(ImportedClip),
    /// Empty time of this length.
    Gap(Time),
    /// A blend across the neighbouring cut.
    Transition {
        /// How far the blend reaches back into the outgoing item.
        in_offset: Time,
        /// How far it reaches forward into the incoming item.
        out_offset: Time,
    },
}

/// One clip of an imported lane.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedClip {
    /// Display name for the timeline rectangle.
    pub name: String,
    /// Index into [`ImportPlan::media`] of the file this clip plays.
    pub media_index: usize,
    /// The portion of that file used, in media time.
    pub source_range: Span,
    /// Markers anchored to the clip, in source time.
    pub markers: Vec<ImportedMarker>,
}

/// One imported marker.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedMarker {
    /// Display name shown on the ruler.
    pub name: String,
    /// Free-form note; OTIO's `comment`.
    pub note: String,
    /// The span covered.
    pub marked_range: Span,
}

/// Reads an OTIO document and describes what the host should create.
///
/// # Errors
///
/// Returns `otio.invalid_document` when the text is not an OTIO timeline,
/// `otio.invalid_time` when a time in it is not exactly representable, and
/// `otio.unsupported` when a timeline carries no time at all to take its
/// timebase from.
pub fn import_document(text: &str) -> Result<ImportPlan> {
    import_timeline(&Timeline::parse(text)?)
}

/// The same, from an already parsed timeline.
///
/// # Errors
///
/// As [`import_document`].
pub fn import_timeline(timeline: &Timeline) -> Result<ImportPlan> {
    let mut notes = Vec::new();
    let frame_rate = timebase(timeline)?;
    let settings = declared_settings(timeline, &mut notes);

    let mut media: Vec<ImportedMedia> = Vec::new();
    let mut tracks = Vec::new();
    for track in &timeline.tracks.children {
        let Some(kind) = track_kind(&track.kind) else {
            notes.push(format!(
                "skipped the {} track {:?}: only Video and Audio tracks are imported",
                track.kind, track.name
            ));
            continue;
        };
        tracks.push(import_track(track, kind, &mut media, &mut notes)?);
    }

    let markers = import_markers(&timeline.tracks.markers)?;

    Ok(ImportPlan {
        media,
        sequence: ImportedSequence {
            name: if timeline.name.is_empty() {
                "Imported".to_owned()
            } else {
                timeline.name.clone()
            },
            frame_rate,
            resolution: settings.resolution,
            sample_rate: settings.sample_rate,
            tracks,
            markers,
        },
        notes,
    })
}

/// Imports one lane, adding any media it introduces to `media`.
fn import_track(
    track: &Track,
    kind: TrackKind,
    media: &mut Vec<ImportedMedia>,
    notes: &mut Vec<String>,
) -> Result<ImportedTrack> {
    if !track.markers.is_empty() {
        notes.push(format!(
            "dropped {} marker(s) on the track {:?}: markers belong to a sequence or a clip",
            track.markers.len(),
            track.name
        ));
    }

    let mut items = Vec::with_capacity(track.children.len());
    for child in &track.children {
        match child {
            Item::Clip(clip) => {
                let source_range = clip
                    .source_range
                    .ok_or_else(|| {
                        invalid_document(
                            "a clip without a source range has no length",
                            "source_range",
                        )
                    })?
                    .to_span()?;
                let reference = clip.active_media_reference()?;
                let Some(file) = reference.as_ref().and_then(schema::MediaReference::file) else {
                    notes.push(format!(
                        "the clip {:?} references no file, so it was imported as a gap",
                        clip.name
                    ));
                    items.push(ImportedItem::Gap(source_range.duration()));
                    continue;
                };
                let imported = media_entry(file, media);
                items.push(ImportedItem::Clip(ImportedClip {
                    name: clip.name.clone(),
                    media_index: imported,
                    source_range,
                    markers: import_markers(&clip.markers)?,
                }));
            }
            Item::Gap(gap) => items.push(ImportedItem::Gap(gap.source_range.to_span()?.duration())),
            Item::Transition(transition) => {
                if transition.transition_type != schema::SMPTE_DISSOLVE {
                    notes.push(format!(
                        "the transition {:?} is a {}, imported as a crossfade: crossfade is the \
                         only transition the model has",
                        transition.name, transition.transition_type
                    ));
                }
                items.push(ImportedItem::Transition {
                    in_offset: transition.in_offset.to_time()?,
                    out_offset: transition.out_offset.to_time()?,
                });
            }
        }
    }

    Ok(ImportedTrack {
        name: track.name.clone(),
        kind,
        items,
    })
}

/// Imports a marker list; OTIO's `comment` is the model's note.
fn import_markers(markers: &[Marker]) -> Result<Vec<ImportedMarker>> {
    markers
        .iter()
        .map(|marker| {
            Ok(ImportedMarker {
                name: marker.name.clone(),
                note: marker.comment.clone(),
                marked_range: marker.marked_range.to_span()?,
            })
        })
        .collect()
}

/// The index of `file` in `media`, adding it if this is its first use.
fn media_entry(file: &str, media: &mut Vec<ImportedMedia>) -> usize {
    let path = relative_media_path(file);
    let name = file_name(&path).to_owned();
    if let Some(index) = media.iter().position(|item| item.path == path) {
        return index;
    }
    media.push(ImportedMedia { path, name });
    media.len() - 1
}

/// The `Track.kind` strings the model has a lane for.
fn track_kind(kind: &str) -> Option<TrackKind> {
    match kind {
        schema::KIND_VIDEO => Some(TrackKind::Video),
        schema::KIND_AUDIO => Some(TrackKind::Audio),
        _ => None,
    }
}

/// The canvas and audio rate, from this plugin's own metadata when the
/// document carries it and from the model's defaults when it does not.
///
/// OTIO stores neither: a timeline has no render resolution and no audio rate,
/// so an OTIO file from another tool can only be imported at the defaults, and
/// says so in the notes.
fn declared_settings(timeline: &Timeline, notes: &mut Vec<String>) -> Settings {
    let mut settings = Settings::default();
    let Some(ours) = timeline.metadata.get(schema::METADATA_NAMESPACE) else {
        notes.push(format!(
            "the document declares no canvas or audio rate, so the sequence was created at \
             {}x{} and {} Hz",
            settings.resolution.width, settings.resolution.height, settings.sample_rate
        ));
        return settings;
    };
    if let Some(resolution) = ours.get("resolution")
        && let (Some(width), Some(height)) = (
            resolution.get("width").and_then(Value::as_u64),
            resolution.get("height").and_then(Value::as_u64),
        )
        && let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height))
        && width > 0
        && height > 0
    {
        settings.resolution = Resolution { width, height };
    }
    if let Some(rate) = ours.get("sample_rate").and_then(Value::as_u64)
        && let Ok(rate) = u32::try_from(rate)
        && rate > 0
    {
        settings.sample_rate = rate;
    }
    settings
}

/// The timebase of the document: the rate of the first time it declares.
///
/// OTIO puts no rate on a timeline, only on the times inside it, so this looks
/// where a rate is most likely to be authoritative first — the timeline's own
/// start, then each track's span — before falling back to the first time on
/// any item.
fn timebase(timeline: &Timeline) -> Result<Rate> {
    if let Some(start) = timeline.global_start_time {
        return Rate::from_f64(start.rate);
    }
    for track in &timeline.tracks.children {
        if let Some(range) = track.source_range {
            return Rate::from_f64(range.duration.rate);
        }
    }
    for track in &timeline.tracks.children {
        for child in &track.children {
            let rate = match child {
                Item::Clip(clip) => clip.source_range.map(|range| range.duration.rate),
                Item::Gap(gap) => Some(gap.source_range.duration.rate),
                Item::Transition(transition) => Some(transition.in_offset.rate),
            };
            if let Some(rate) = rate {
                return Rate::from_f64(rate);
            }
        }
    }
    Err(Error::new(
        codes::UNSUPPORTED,
        "the document carries no time at all, so it has no timebase to import at",
    ))
}

/// Turns a `target_url` into a path the project model can hold.
///
/// A media path in Subordinate is project-relative with `/` separators, so
/// that a project folder copied to another machine still relinks (docs/PLAN.md
/// §5.6). OTIO writers do not oblige: OTIO's own samples use `file://` URLs
/// and Kdenlive writes the absolute path of the file on the machine that
/// exported it. An absolute path from another machine is not a path this
/// project can store, so it is reduced to its file name and the host relinks
/// it like any other missing media.
fn relative_media_path(target_url: &str) -> String {
    let stripped = target_url
        .strip_prefix("file://localhost")
        .or_else(|| target_url.strip_prefix("file://"))
        .unwrap_or(target_url);
    let decoded = percent_decode(stripped).replace('\\', "/");
    let trimmed = decoded.trim();

    let absolute = trimmed.starts_with('/')
        || trimmed.split_once(':').is_some_and(|(prefix, _)| {
            prefix.len() == 1 && prefix.chars().all(char::is_alphabetic)
        });
    let traversing = trimmed
        .split('/')
        .any(|segment| segment == ".." || segment == "." || segment.is_empty());

    if trimmed.is_empty() {
        return "unnamed".to_owned();
    }
    if absolute || traversing {
        return file_name(trimmed).to_owned();
    }
    trimmed.to_owned()
}

/// The last `/`-separated segment, or a placeholder when there is none.
fn file_name(path: &str) -> &str {
    let name = path.rsplit('/').find(|segment| !segment.is_empty());
    name.unwrap_or("unnamed")
}

/// Decodes the `%XX` escapes a `file://` URL may carry. Anything that is not a
/// well-formed escape is left exactly as it was.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "two hex digits are one byte by construction"
                )]
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_is_kept_and_anything_else_is_reduced_to_its_file_name() {
        assert_eq!(relative_media_path("footage/a.mov"), "footage/a.mov");
        assert_eq!(relative_media_path("/home/ed/a.mov"), "a.mov");
        assert_eq!(relative_media_path("file:///home/ed/a%20b.mov"), "a b.mov");
        assert_eq!(relative_media_path("C:\\media\\a.mov"), "a.mov");
        assert_eq!(relative_media_path("../outside.mov"), "outside.mov");
        assert_eq!(relative_media_path(""), "unnamed");
    }

    #[test]
    fn a_malformed_escape_is_left_alone() {
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn a_timeline_with_no_times_has_no_timebase() {
        let timeline = Timeline::parse(
            r#"{"OTIO_SCHEMA":"Timeline.1","name":"empty","tracks":{"OTIO_SCHEMA":"Stack.1","children":[]}}"#,
        )
        .unwrap();
        assert_eq!(timebase(&timeline).unwrap_err().code, codes::UNSUPPORTED);
    }

    /// The MVP flattens OTIO's stack of tracks into one list of lanes, so a
    /// stack nested inside a track has nowhere to go. Refusing the document is
    /// the honest answer: flattening it silently would move everything after
    /// it.
    #[test]
    fn a_stack_nested_in_a_track_is_refused_rather_than_flattened() {
        let nested = r#"{
            "OTIO_SCHEMA": "Timeline.1",
            "name": "nested",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "children": [{
                    "OTIO_SCHEMA": "Track.1",
                    "name": "V1",
                    "kind": "Video",
                    "children": [{ "OTIO_SCHEMA": "Stack.1", "children": [] }]
                }]
            }
        }"#;
        let error = crate::import_document(nested).unwrap_err();
        assert_eq!(error.code, codes::INVALID_DOCUMENT);
    }

    /// A clip is placed positionally, so one with no source range has no
    /// length and every item after it would move.
    #[test]
    fn a_clip_without_a_source_range_is_refused() {
        let lengthless = r#"{
            "OTIO_SCHEMA": "Timeline.1",
            "name": "lengthless",
            "global_start_time": { "rate": 25.0, "value": 0.0 },
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "children": [{
                    "OTIO_SCHEMA": "Track.1",
                    "name": "V1",
                    "kind": "Video",
                    "children": [{ "OTIO_SCHEMA": "Clip.1", "name": "no range" }]
                }]
            }
        }"#;
        let error = crate::import_document(lengthless).unwrap_err();
        assert_eq!(error.code, codes::INVALID_DOCUMENT);
        assert_eq!(error.details["field"], "source_range");
    }
}
