//! Importing an OTIO file another editor wrote.
//!
//! `fixtures/kdenlive.otio` is a timeline in the shape Kdenlive's native OTIO
//! export produces. It was written against Kdenlive's exporter,
//! `src/otio/otioexport.cpp` in the KDE/kdenlive repository, which is what
//! makes it a fixture rather than a guess: every idiom in it is one that file
//! writes, and each is something a hand-rolled fixture would have got wrong.
//!
//! - the timeline carries a `kdenlive` metadata namespace and an empty name;
//! - `global_start_time` is null, so the timebase has to come from elsewhere;
//! - every track declares a `source_range`, and the tracks are ordered video
//!   first, audio after;
//! - clips are `Clip.2`: a `media_references` map plus the key of the active
//!   entry, not `Clip.1`'s single `media_reference`;
//! - an `ExternalReference` carries the absolute path of the file on the
//!   machine that exported it, spaces and all, not a project-relative one;
//! - a colour clip is a `GeneratorReference` with a `kdenlive:SolidColor`
//!   kind, which names no file at all;
//! - guides are markers on the stack, clip markers are on their clip, and both
//!   last exactly one frame and carry their text in `comment`;
//! - a mix is a `Transition` named `Transition` with asymmetric offsets;
//! - there is a `Subtitle` track, which is not a lane this model has.

use otio_core::import::{ImportedItem, TrackKind};
use otio_core::import_document;

const KDENLIVE: &str = include_str!("fixtures/kdenlive.otio");

#[test]
fn a_kdenlive_export_imports_with_its_cut_intact() {
    let plan = import_document(KDENLIVE).expect("a Kdenlive export imports");

    // No `global_start_time`, so the timebase came from the first track span.
    assert_eq!(plan.sequence.frame_rate.numerator(), 25);
    assert_eq!(plan.sequence.frame_rate.denominator(), 1);
    // The timeline has no name, and OTIO stores no canvas or audio rate, so
    // the sequence falls back to the model's defaults and says so.
    assert_eq!(plan.sequence.name, "Imported");
    assert_eq!(plan.sequence.resolution.width, 1920);
    assert_eq!(plan.sequence.sample_rate, 48_000);

    // The `Subtitle` track is not imported; the other three are.
    let tracks: Vec<(&str, TrackKind)> = plan
        .sequence
        .tracks
        .iter()
        .map(|track| (track.name.as_str(), track.kind))
        .collect();
    assert_eq!(
        tracks,
        [
            ("V2", TrackKind::Video),
            ("V1", TrackKind::Video),
            ("A1", TrackKind::Audio),
        ]
    );

    // An absolute path from someone else's machine cannot be stored as a
    // project-relative path, so it comes in under its file name; the escaped
    // space in the second one is decoded rather than kept.
    let media: Vec<&str> = plan.media.iter().map(|item| item.path.as_str()).collect();
    assert_eq!(media, ["interview.mp4", "b roll.mp4"]);
    // The same file used on two tracks is one media item, referenced twice.
    assert_eq!(plan.media.len(), 2);

    let v1 = &plan.sequence.tracks[1].items;
    assert_eq!(v1.len(), 3);
    let ImportedItem::Clip(interview) = &v1[0] else {
        panic!("the first item on V1 is a clip");
    };
    assert_eq!(interview.media_index, 0);
    assert_eq!(interview.source_range.start().value(), 25);
    assert_eq!(interview.source_range.duration().value(), 120);
    // A clip marker keeps its text, which Kdenlive writes as `comment`, and
    // its position in source time.
    assert_eq!(interview.markers.len(), 1);
    assert_eq!(interview.markers[0].note, "good take");
    assert_eq!(interview.markers[0].marked_range.start().value(), 40);

    let ImportedItem::Transition {
        in_offset,
        out_offset,
    } = &v1[1]
    else {
        panic!("the second item on V1 is a transition");
    };
    assert_eq!((in_offset.value(), out_offset.value()), (10, 15));

    let ImportedItem::Clip(b_roll) = &v1[2] else {
        panic!("the third item on V1 is a clip");
    };
    assert_eq!(b_roll.media_index, 1);

    // The audio track plays the same file as the video track.
    let ImportedItem::Clip(audio) = &plan.sequence.tracks[2].items[0] else {
        panic!("the audio track holds a clip");
    };
    assert_eq!(audio.media_index, 0);

    // A guide becomes a sequence marker, one frame long, with its text.
    assert_eq!(plan.sequence.markers.len(), 1);
    assert_eq!(plan.sequence.markers[0].note, "second scene starts here");
    assert_eq!(plan.sequence.markers[0].marked_range.start().value(), 75);
    assert_eq!(plan.sequence.markers[0].marked_range.duration().value(), 1);
}

#[test]
fn what_the_model_cannot_hold_becomes_a_gap_or_a_note() {
    let plan = import_document(KDENLIVE).expect("a Kdenlive export imports");

    // V2 is a gap Kdenlive wrote, then a colour generator this model has no
    // clip for. The generator becomes a gap of its own length, so everything
    // after it stays where it was.
    let v2 = &plan.sequence.tracks[0].items;
    assert_eq!(v2.len(), 2);
    let (ImportedItem::Gap(blank), ImportedItem::Gap(generator)) = (&v2[0], &v2[1]) else {
        panic!("V2 is a gap and a generator imported as a gap");
    };
    assert_eq!(blank.value(), 50);
    assert_eq!(generator.value(), 100);

    // Nothing is dropped quietly: the skipped subtitle track, the generator
    // and the guessed settings are all reported.
    let notes = plan.notes.join("\n");
    assert!(notes.contains("Subtitle"), "{notes}");
    assert!(notes.contains("colour"), "{notes}");
    assert!(notes.contains("1920x1080"), "{notes}");
}

#[test]
fn a_document_that_is_not_a_timeline_is_refused_by_code() {
    let clip = r#"{"OTIO_SCHEMA": "Clip.2", "name": "lonely"}"#;
    assert_eq!(
        import_document(clip).unwrap_err().code,
        otio_core::codes::INVALID_DOCUMENT
    );
    assert_eq!(
        import_document("{}").unwrap_err().code,
        otio_core::codes::INVALID_DOCUMENT
    );
    assert_eq!(
        import_document("not json at all").unwrap_err().code,
        otio_core::codes::INVALID_DOCUMENT
    );
}
