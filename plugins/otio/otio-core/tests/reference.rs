//! Importing OTIO written by the reference implementation.
//!
//! `tests/roundtrip.rs` proves that this plugin agrees with itself, which is
//! the weaker half of interchange: a format is only interchange if a file some
//! other implementation wrote comes in. These three fixtures were produced by
//! the reference OpenTimelineIO Python library (0.18.1) and committed
//! unedited, together with
//! `plugins/otio/scripts/generate-reference-fixtures.py`, which regenerates
//! them from the pinned upstream sources.
//!
//! - `reference_multitrack.otio` is `tests/sample_data/multiple_track.otio`
//!   from the reference repository, read and written back by its `otio_json`
//!   adapter: three video tracks, `file://` URLs, gaps, a track carrying its
//!   own `source_range`, and one file used on two tracks.
//! - `reference_nucoda_edl.otio` is a CMX 3600 EDL put through the `cmx_3600`
//!   adapter. Its events name their media with `* FROM FILE`, so the clips
//!   arrive with `ExternalReference`s holding Windows paths, and the timeline
//!   has no `global_start_time` — the timebase has to come from the track.
//! - `reference_screening_edl.otio` is a longer EDL conversion whose events
//!   name no file at all: every clip is a `MissingReference`, three carry
//!   `* LOC` markers, and the whole thing has to import as the cut it is
//!   without inventing media.

use otio_core::import::{ImportedItem, TrackKind};
use otio_core::import_document;
use otio_core::time::Time;

const MULTITRACK: &str = include_str!("fixtures/reference_multitrack.otio");
const NUCODA: &str = include_str!("fixtures/reference_nucoda_edl.otio");
const SCREENING: &str = include_str!("fixtures/reference_screening_edl.otio");

/// The durations of a lane's items, whatever kind each one is.
fn item_durations(items: &[ImportedItem]) -> Vec<i64> {
    items
        .iter()
        .map(|item| match item {
            ImportedItem::Clip(clip) => clip.source_range.duration().value(),
            ImportedItem::Gap(gap) => gap.value(),
            ImportedItem::Transition {
                in_offset,
                out_offset,
            } => in_offset.value() + out_offset.value(),
        })
        .collect()
}

#[test]
fn a_shipped_sample_timeline_imports_with_its_cut_intact() {
    let plan = import_document(MULTITRACK).expect("a reference sample timeline imports");

    // `global_start_time` is 24 fps exactly, and it is exact on the way in:
    // OTIO wrote it as the double 24.0.
    assert_eq!(plan.sequence.name, "Figure 3 - Multiple Tracks");
    assert_eq!(plan.sequence.frame_rate.numerator(), 24);
    assert_eq!(plan.sequence.frame_rate.denominator(), 1);

    let tracks: Vec<(&str, TrackKind)> = plan
        .sequence
        .tracks
        .iter()
        .map(|track| (track.name.as_str(), track.kind))
        .collect();
    assert_eq!(
        tracks,
        [
            ("Track-001", TrackKind::Video),
            ("Track-002", TrackKind::Video),
            ("Track-003", TrackKind::Video),
        ]
    );

    // `file:///folder/punchline.mov` is played on two tracks and is one media
    // item; every URL is reduced to a project-relative name.
    let media: Vec<&str> = plan.media.iter().map(|item| item.path.as_str()).collect();
    assert_eq!(
        media,
        ["titles.mov", "wind-up.mov", "credits.mov", "punchline.mov"]
    );

    // Track-001: two clips, a four-frame gap, then a clip.
    let first = &plan.sequence.tracks[0].items;
    assert_eq!(item_durations(first), [3, 6, 4, 6]);
    let ImportedItem::Clip(titles) = &first[0] else {
        panic!("the lane opens on a clip");
    };
    assert_eq!(titles.name, "Clip-001");
    assert_eq!(titles.media_index, 0);
    assert_eq!(titles.source_range.start().value(), 3);
    assert!(matches!(first[2], ImportedItem::Gap(_)));

    // Track-002 opens on a gap, so its clip starts seven frames in.
    assert_eq!(item_durations(&plan.sequence.tracks[1].items), [7, 9]);

    // Track-003 declares its own `source_range`, which the model has no place
    // for: the lane is imported positionally and the clip on it is unchanged.
    let third = &plan.sequence.tracks[2].items;
    let ImportedItem::Clip(punchline) = &third[0] else {
        panic!("the third lane holds one clip");
    };
    assert_eq!(punchline.media_index, 3);
    assert_eq!(punchline.source_range.start().value(), 100);
    assert_eq!(punchline.source_range.duration().value(), 9);

    // OTIO stores no canvas and no audio rate, so both were guessed and said
    // so; nothing else about this document was lossy.
    assert_eq!(plan.notes.len(), 1, "{:?}", plan.notes);
    assert!(plan.notes[0].contains("1920x1080"), "{:?}", plan.notes);
}

#[test]
fn an_edl_converted_by_the_reference_adapter_imports() {
    let plan = import_document(NUCODA).expect("a converted EDL imports");

    // The timeline has no `global_start_time`; the rate came from the track's
    // own span, which the adapter writes at the EDL's 24 fps.
    assert_eq!(plan.sequence.name, "Nucoda_Example.01");
    assert_eq!(plan.sequence.frame_rate.numerator(), 24);
    assert_eq!(plan.sequence.frame_rate.denominator(), 1);
    assert_eq!(plan.sequence.tracks.len(), 1);
    assert_eq!(plan.sequence.tracks[0].name, "V");
    assert_eq!(plan.sequence.tracks[0].kind, TrackKind::Video);

    // `* FROM FILE: S:\path\to\...` is an absolute path on someone else's
    // machine, so each one comes in under its file name to be relinked.
    let media: Vec<&str> = plan.media.iter().map(|item| item.path.as_str()).collect();
    assert_eq!(
        media,
        ["ZZ100_501.take_1.0001.exr", "ZZ100_502A.take_2.0101.exr"]
    );

    // Source timecode is kept exactly: 01:00:04:05 at 24 fps is frame 86501,
    // and the two events are 31 and 50 frames long.
    let items = &plan.sequence.tracks[0].items;
    let starts_and_lengths: Vec<(i64, i64)> = items
        .iter()
        .map(|item| {
            let ImportedItem::Clip(clip) = item else {
                panic!("every event on this lane is a clip");
            };
            (
                clip.source_range.start().value(),
                clip.source_range.duration().value(),
            )
        })
        .collect();
    assert_eq!(starts_and_lengths, [(86_501, 31), (86_557, 50)]);
    assert_eq!(
        items
            .iter()
            .map(|item| match item {
                ImportedItem::Clip(clip) => clip.name.as_str(),
                _ => panic!("every event on this lane is a clip"),
            })
            .collect::<Vec<_>>(),
        ["take_1", "take_2"]
    );
}

#[test]
fn an_edl_whose_events_name_no_file_keeps_its_cut_and_reports_the_loss() {
    let plan = import_document(SCREENING).expect("an EDL with no media references imports");

    assert_eq!(plan.sequence.name, "Example_Screening.01");
    assert_eq!(plan.sequence.frame_rate.numerator(), 24);
    // No event names a file, so there is no media to file in the bin.
    assert!(plan.media.is_empty(), "{:?}", plan.media);

    // Every event still occupies exactly the time it occupied in the EDL, so
    // the cut is the one the conform room made: nine events, 1049 frames.
    let durations = item_durations(&plan.sequence.tracks[0].items);
    assert_eq!(durations, [31, 50, 28, 115, 101, 161, 170, 136, 257]);
    assert_eq!(durations.iter().sum::<i64>(), 1049);
    assert!(
        plan.sequence.tracks[0]
            .items
            .iter()
            .all(|item| matches!(item, ImportedItem::Gap(_)))
    );
    // A gap length is still exact time, not a float that happened to land.
    assert_eq!(
        plan.sequence.tracks[0].items[0],
        ImportedItem::Gap(Time::new(31, plan.sequence.frame_rate))
    );

    // Nothing went quietly: nine clips with no file, and the three `* LOC`
    // markers that went with two of them.
    let no_file = plan
        .notes
        .iter()
        .filter(|note| note.contains("references no file"))
        .count();
    assert_eq!(no_file, 9, "{:?}", plan.notes);
    let dropped_markers: i64 = plan
        .notes
        .iter()
        .filter(|note| note.contains("marker(s) on the clip"))
        .filter_map(|note| note.split_whitespace().nth(1)?.parse::<i64>().ok())
        .sum();
    assert_eq!(dropped_markers, 3, "{:?}", plan.notes);
}
