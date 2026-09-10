//! Turning one Subordinate sequence into an OTIO timeline.
//!
//! The mapping is the one the model was designed around (docs/PLAN.md §5.1):
//!
//! | Subordinate            | OTIO                                     |
//! | ---------------------- | ---------------------------------------- |
//! | `Sequence`             | `Timeline`, whose `tracks` is a `Stack`  |
//! | `Track`                | `Track`, `kind` `Video` or `Audio`       |
//! | `Clip`                 | `Clip` with an `ExternalReference`       |
//! | `Gap`                  | `Gap`                                    |
//! | `Transition::Crossfade`| `Transition`, `SMPTE_Dissolve`           |
//! | `Marker`               | `Marker`, on the `Stack` or on the `Clip`|
//!
//! What OTIO has no field for goes into the `subordinate` metadata namespace:
//! the canvas resolution, the audio sample rate and the identifiers, so that a
//! document this plugin wrote imports back as the same sequence rather than as
//! a sequence at guessed settings.
//!
//! **Effects are not exported.** Neither the per-clip parameters the MVP
//! supports — opacity, transform, gain, fades — nor plugin effects appear in
//! the output, because OTIO's `Effect` is a name and an untyped parameter bag
//! with no agreed vocabulary for any of them, and an export that wrote them
//! under invented names would import into other tools as nothing at all. The
//! cut survives the round trip; the look does not.

use serde_json::{Map, Value, json};

use crate::error::Result;
use crate::project::{self, Project};
use crate::schema::{
    self, Clip, Gap, Item, Marker, MediaReference, RationalTime, Stack, TimeRange, Timeline, Track,
    Transition,
};
use crate::time::{Rate, Span, Time};

/// Exports `sequence` of `project` as an OTIO timeline.
///
/// # Errors
///
/// Returns `otio.invalid_project` when a rate in the sequence has a zero part,
/// and `otio.invalid_time` when a span is negative or a duration cannot be
/// summed exactly at the sequence rate.
pub fn export_sequence(project: &Project, sequence: &project::Sequence) -> Result<Timeline> {
    let rate = sequence.settings.frame_rate.to_rate()?;
    let mut tracks = Vec::with_capacity(sequence.tracks.len());
    for track in &sequence.tracks {
        tracks.push(export_track(project, track, rate)?);
    }

    Ok(Timeline {
        schema: schema::TIMELINE_SCHEMA.to_owned(),
        name: sequence.name.clone(),
        global_start_time: Some(RationalTime::from_time(Time::zero(rate))),
        tracks: Stack {
            schema: schema::STACK_SCHEMA.to_owned(),
            name: "tracks".to_owned(),
            children: tracks,
            markers: export_markers(&sequence.markers)?,
            enabled: true,
            metadata: Map::new(),
        },
        metadata: namespaced(json!({
            "project_id": project.id,
            "project_name": project.name,
            "sequence_id": sequence.id,
            "resolution": {
                "width": sequence.settings.resolution.width,
                "height": sequence.settings.resolution.height,
            },
            "sample_rate": sequence.settings.sample_rate,
        })),
    })
}

/// Exports one lane, and the span it covers.
fn export_track(project: &Project, track: &project::Track, rate: Rate) -> Result<Track> {
    let mut children = Vec::with_capacity(track.items.len());
    let mut duration = Time::zero(rate);
    for item in &track.items {
        match item {
            project::Item::Clip(clip) => {
                let source_range = clip.source_range.to_span()?;
                duration = duration.checked_add(source_range.duration())?;
                children.push(Item::Clip(Box::new(export_clip(
                    project,
                    clip,
                    source_range,
                )?)));
            }
            project::Item::Gap(gap) => {
                let length = gap.duration.to_time()?;
                duration = duration.checked_add(length)?;
                children.push(Item::Gap(Gap {
                    name: String::new(),
                    // OTIO carries a gap's length as a range starting at zero.
                    source_range: TimeRange::from_span(Span::new(
                        Time::zero(length.rate()),
                        length,
                    )?),
                    metadata: Map::new(),
                }));
            }
            // A transition consumes no track time of its own, so it adds
            // nothing to the running duration.
            project::Item::Transition(project::Transition::Crossfade {
                in_offset,
                out_offset,
            }) => children.push(Item::Transition(Transition {
                name: "Crossfade".to_owned(),
                transition_type: schema::SMPTE_DISSOLVE.to_owned(),
                in_offset: RationalTime::from_time(in_offset.to_time()?),
                out_offset: RationalTime::from_time(out_offset.to_time()?),
                metadata: Map::new(),
            })),
        }
    }

    Ok(Track {
        schema: schema::TRACK_SCHEMA.to_owned(),
        name: track.name.clone(),
        kind: match track.kind {
            project::TrackKind::Video => schema::KIND_VIDEO.to_owned(),
            project::TrackKind::Audio => schema::KIND_AUDIO.to_owned(),
        },
        source_range: Some(TimeRange::from_span(Span::new(Time::zero(rate), duration)?)),
        children,
        markers: Vec::new(),
        // A muted lane is one that does not contribute, which is exactly what
        // OTIO's `enabled` says. Locking has no OTIO counterpart and is left
        // in the metadata.
        enabled: !track.muted,
        metadata: namespaced(json!({ "track_id": track.id, "locked": track.locked })),
    })
}

/// Exports one clip, with the media reference the project's bin gives it.
fn export_clip(project: &Project, clip: &project::Clip, source_range: Span) -> Result<Clip> {
    let media = project.media_item(&clip.media);
    let media_reference = media.map(|item| MediaReference::external(item.path.clone()));
    let name = if clip.name.is_empty() {
        media.map_or_else(String::new, |item| item.name.clone())
    } else {
        clip.name.clone()
    };

    Ok(Clip {
        name,
        source_range: Some(TimeRange::from_span(source_range)),
        media_reference,
        media_references: Map::new(),
        active_media_reference_key: None,
        markers: export_markers(&clip.markers)?,
        enabled: true,
        metadata: namespaced(json!({ "clip_id": clip.id, "media_id": clip.media })),
    })
}

/// Exports a marker list, preserving the note as OTIO's `comment`.
fn export_markers(markers: &[project::Marker]) -> Result<Vec<Marker>> {
    markers
        .iter()
        .map(|marker| {
            Ok(Marker {
                schema: schema::MARKER_SCHEMA.to_owned(),
                name: marker.name.clone(),
                marked_range: TimeRange::from_span(marker.marked_range.to_span()?),
                color: "RED".to_owned(),
                comment: marker.note.clone(),
                metadata: namespaced(json!({ "marker_id": marker.id })),
            })
        })
        .collect()
}

/// Wraps a value in this plugin's metadata namespace, as OTIO asks writers to
/// do with anything outside the standard schema.
fn namespaced(value: Value) -> Map<String, Value> {
    let mut metadata = Map::new();
    metadata.insert(schema::METADATA_NAMESPACE.to_owned(), value);
    metadata
}
