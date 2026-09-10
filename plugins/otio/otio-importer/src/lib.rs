//! The first-party OpenTimelineIO importer.
//!
//! It implements the `importer` world: [`supported_extensions`] tells the host
//! which files to offer it, and `import` reads one and describes what it found.
//! The description is applied by the host through the Command API, so an
//! import lands on the undo stack like any other edit and every identifier is
//! minted by the host (docs/PLAN.md §3, §6.2).
//!
//! All of the reading and all of the conversion is in `otio-core`; this crate
//! is the mapping onto the WIT records, which is why it is so short.
//!
//! Build it with `cargo build --release --target wasm32-wasip2`.

pub mod bindings;

use otio_core::error::Error;
use otio_core::import::{ImportPlan, ImportedItem, ImportedMarker, ImportedTrack, TrackKind};
use otio_core::time::{Rate, Span, Time};

use crate::bindings::exports::subordinate::plugin::importer_api::{
    ClipSpec, Guest, MarkerSpec, MediaOrSequenceSpec, MediaRef, MediaSpec, SequenceSpec,
    TrackItemSpec, TrackSpec, TransitionSpec,
};
use crate::bindings::subordinate::plugin::command_api::{
    self, LogLevel, Resolution, TrackKind as WitTrackKind,
};
use crate::bindings::subordinate::plugin::types::{
    Detail, Error as WitError, Rational, RationalTime, TimeRange,
};

/// The file extensions this importer handles.
///
/// `otio` is OpenTimelineIO's own; `otioz` (a zipped bundle) and `otiod` (a
/// directory bundle) are not, because they are archives rather than JSON.
pub const EXTENSIONS: [&str; 1] = ["otio"];

/// The `importer` world implementation.
pub struct Plugin;

impl Guest for Plugin {
    fn supported_extensions() -> Vec<String> {
        EXTENSIONS
            .iter()
            .map(|extension| (*extension).to_owned())
            .collect()
    }

    fn import(path: String) -> Result<Vec<MediaOrSequenceSpec>, WitError> {
        let text = std::fs::read_to_string(&path).map_err(|error| {
            wit_error(
                &Error::new(
                    otio_core::codes::INVALID_DOCUMENT,
                    format!("could not read the file: {error}"),
                )
                .with("path", &path),
            )
        })?;
        let plan = otio_core::import_document(&text).map_err(|error| wit_error(&error))?;
        for note in &plan.notes {
            command_api::log(LogLevel::Info, &format!("otio import: {note}"));
        }
        Ok(specs(&plan))
    }
}

/// The component's exported entry points.
///
/// Only on wasm: an interface export's symbol name is the full WIT name,
/// `subordinate:plugin/...#...`, which a host linker's export list cannot
/// parse. The `rlib` half of this crate is built for the host so the mapping
/// below can be unit-tested, and it needs no component glue to do that.
#[cfg(target_family = "wasm")]
mod component {
    #![allow(
        unsafe_code,
        reason = "the generated export glue is `extern \"C\"` shims with exported symbol names"
    )]

    use super::Plugin;

    crate::bindings::export!(Plugin);
}

/// Maps a finished plan onto the records the host applies.
///
/// Media comes first because a clip refers to a media entry by its index in
/// this same list.
#[must_use]
pub fn specs(plan: &ImportPlan) -> Vec<MediaOrSequenceSpec> {
    let mut specs: Vec<MediaOrSequenceSpec> = plan
        .media
        .iter()
        .map(|media| {
            MediaOrSequenceSpec::Media(MediaSpec {
                path: media.path.clone(),
                name: media.name.clone(),
            })
        })
        .collect();

    specs.push(MediaOrSequenceSpec::Sequence(SequenceSpec {
        name: plan.sequence.name.clone(),
        frame_rate: rational(plan.sequence.frame_rate),
        resolution: Resolution {
            width: plan.sequence.resolution.width,
            height: plan.sequence.resolution.height,
        },
        sample_rate: plan.sequence.sample_rate,
        tracks: plan.sequence.tracks.iter().map(track_spec).collect(),
        markers: plan.sequence.markers.iter().map(marker_spec).collect(),
    }));
    specs
}

/// One lane.
fn track_spec(track: &ImportedTrack) -> TrackSpec {
    TrackSpec {
        name: track.name.clone(),
        kind: match track.kind {
            TrackKind::Video => WitTrackKind::Video,
            TrackKind::Audio => WitTrackKind::Audio,
        },
        items: track.items.iter().map(item_spec).collect(),
    }
}

/// One item of a lane.
fn item_spec(item: &ImportedItem) -> TrackItemSpec {
    match item {
        ImportedItem::Clip(clip) => TrackItemSpec::Clip(ClipSpec {
            name: clip.name.clone(),
            // Every clip a plan produces refers to media the same import
            // creates: `existing` is for a file already in the bins, which an
            // OTIO document has no way to name.
            media: MediaRef::Imported(u32::try_from(clip.media_index).unwrap_or(u32::MAX)),
            source_range: time_range(clip.source_range),
            markers: clip.markers.iter().map(marker_spec).collect(),
        }),
        ImportedItem::Gap(duration) => TrackItemSpec::Gap(rational_time(*duration)),
        ImportedItem::Transition {
            in_offset,
            out_offset,
        } => TrackItemSpec::Transition(TransitionSpec {
            in_offset: rational_time(*in_offset),
            out_offset: rational_time(*out_offset),
        }),
    }
}

/// One marker.
fn marker_spec(marker: &ImportedMarker) -> MarkerSpec {
    MarkerSpec {
        name: marker.name.clone(),
        note: marker.note.clone(),
        marked_range: time_range(marker.marked_range),
    }
}

/// An exact rate, unreduced denominators and all.
fn rational(rate: Rate) -> Rational {
    Rational {
        numerator: rate.numerator(),
        denominator: rate.denominator(),
    }
}

/// An exact instant.
fn rational_time(time: Time) -> RationalTime {
    RationalTime {
        value: time.value(),
        rate: rational(time.rate()),
    }
}

/// An exact span.
fn time_range(span: Span) -> TimeRange {
    TimeRange {
        start: rational_time(span.start()),
        duration: rational_time(span.duration()),
    }
}

/// Carries a failure across the component boundary with its code intact.
fn wit_error(error: &Error) -> WitError {
    WitError {
        code: error.code.to_owned(),
        message: error.message.clone(),
        details: error
            .details
            .iter()
            .map(|(key, value)| Detail {
                key: key.clone(),
                // A detail's value is a JSON fragment, so a string detail is a
                // quoted string.
                value: format!("{value:?}"),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-track document with a gap, a crossfade, a clip marker and a
    /// sequence marker: everything the mapping has a case for.
    const DOCUMENT: &str = include_str!("../../otio-core/tests/fixtures/roundtrip.otio");

    #[test]
    fn media_is_described_before_the_sequence_that_plays_it() {
        let plan = otio_core::import_document(DOCUMENT).unwrap();
        let specs = specs(&plan);
        let media = specs
            .iter()
            .take_while(|spec| matches!(spec, MediaOrSequenceSpec::Media(_)))
            .count();
        assert_eq!(media, plan.media.len());
        assert_eq!(specs.len(), media + 1);
        assert!(matches!(
            specs.last(),
            Some(MediaOrSequenceSpec::Sequence(_))
        ));
    }

    #[test]
    fn every_clip_refers_to_a_media_entry_of_this_same_import() {
        let plan = otio_core::import_document(DOCUMENT).unwrap();
        let specs = specs(&plan);
        let media_count = u32::try_from(plan.media.len()).unwrap();
        let Some(MediaOrSequenceSpec::Sequence(sequence)) = specs.last() else {
            panic!("the last spec is the sequence");
        };
        let mut clips = 0;
        for track in &sequence.tracks {
            for item in &track.items {
                if let TrackItemSpec::Clip(clip) = item {
                    clips += 1;
                    let MediaRef::Imported(index) = clip.media else {
                        panic!("an imported clip refers to imported media");
                    };
                    assert!(index < media_count);
                }
            }
        }
        assert!(clips > 0);
    }

    #[test]
    fn times_cross_as_exact_fractions() {
        let plan = otio_core::import_document(DOCUMENT).unwrap();
        let specs = specs(&plan);
        let Some(MediaOrSequenceSpec::Sequence(sequence)) = specs.last() else {
            panic!("the last spec is the sequence");
        };
        assert_eq!(sequence.frame_rate.numerator, 24_000);
        assert_eq!(sequence.frame_rate.denominator, 1001);
        for track in &sequence.tracks {
            for item in &track.items {
                if let TrackItemSpec::Clip(clip) = item {
                    assert_eq!(clip.source_range.duration.rate.denominator, 1001);
                }
            }
        }
    }

    #[test]
    fn the_extension_list_is_what_the_host_filters_on() {
        assert_eq!(Plugin::supported_extensions(), vec!["otio".to_owned()]);
    }
}
