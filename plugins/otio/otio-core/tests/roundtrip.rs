//! Export a project, import what came out, and check nothing moved.
//!
//! `fixtures/project.json` is a project in the shape `project.get` answers
//! with, at 23.976 fps so that the exact-rate handling is exercised rather
//! than assumed. `fixtures/roundtrip.otio` is the document the exporter
//! produces from it, committed as a golden file: re-record it with
//! `OTIO_UPDATE_FIXTURES=1 cargo test -p otio-core` and read the diff.

use std::path::Path;

use otio_core::import::{ImportedItem, TrackKind};
use otio_core::project::Project;
use otio_core::{export_sequence, import_document};

const PROJECT: &str = include_str!("fixtures/project.json");

fn project() -> Project {
    serde_json::from_str(PROJECT).expect("the fixture is a project")
}

fn exported() -> String {
    let project = project();
    let sequence = project
        .sequence("Edit")
        .expect("the fixture has an Edit sequence");
    export_sequence(&project, sequence)
        .expect("the fixture exports")
        .to_json()
        .expect("the document serialises")
}

#[test]
fn the_exported_document_is_the_committed_golden_file() {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/roundtrip.otio");
    let document = exported();
    if std::env::var_os("OTIO_UPDATE_FIXTURES").is_some() {
        std::fs::write(&golden, &document).expect("the golden file is writable");
    }
    let expected = std::fs::read_to_string(&golden).expect("the golden file exists");
    assert_eq!(
        document, expected,
        "the export changed; re-record with OTIO_UPDATE_FIXTURES=1 if that was intended"
    );
}

#[test]
fn the_document_is_openttimelineio_shaped() {
    let document: serde_json::Value = serde_json::from_str(&exported()).expect("valid JSON");
    assert_eq!(document["OTIO_SCHEMA"], "Timeline.1");
    assert_eq!(document["name"], "Edit");
    assert_eq!(document["tracks"]["OTIO_SCHEMA"], "Stack.1");

    let tracks = document["tracks"]["children"]
        .as_array()
        .expect("two tracks");
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0]["kind"], "Video");
    assert_eq!(tracks[1]["kind"], "Audio");
    // The audio track is muted in the project, which is what OTIO's `enabled`
    // says.
    assert_eq!(tracks[0]["enabled"], true);
    assert_eq!(tracks[1]["enabled"], false);

    let video = tracks[0]["children"].as_array().expect("five items");
    let schemas: Vec<&str> = video
        .iter()
        .map(|item| {
            item["OTIO_SCHEMA"]
                .as_str()
                .expect("every item names its schema")
        })
        .collect();
    assert_eq!(
        schemas,
        ["Clip.1", "Transition.1", "Clip.1", "Gap.1", "Clip.1"]
    );
    assert_eq!(video[1]["transition_type"], "SMPTE_Dissolve");
    assert_eq!(
        video[0]["media_reference"]["OTIO_SCHEMA"],
        "ExternalReference.1"
    );
    assert_eq!(
        video[0]["media_reference"]["target_url"],
        "footage/wide.mov"
    );

    // The sequence marker rides on the stack, where OTIO — and Kdenlive —
    // put timeline-wide markers; the clip marker rides on its clip.
    assert_eq!(document["tracks"]["markers"][0]["name"], "act one");
    assert_eq!(video[2]["markers"][0]["name"], "eyeline");
    assert_eq!(
        video[2]["markers"][0]["comment"],
        "check the eyeline against the wide"
    );

    // 23.976 fps is written as OTIO writes it, and the durations are whole
    // frames.
    let rate = video[0]["source_range"]["duration"]["rate"]
        .as_f64()
        .expect("a rate");
    assert!((rate - 24_000.0 / 1001.0).abs() < 1e-9, "{rate}");
    assert_eq!(video[0]["source_range"]["duration"]["value"], 48.0);
}

#[test]
fn no_effect_or_clip_parameter_reaches_the_document() {
    let document = exported();
    for parameter in [
        "opacity",
        "transform",
        "gain",
        "fade_in",
        "fade_out",
        "effects",
    ] {
        assert!(
            !document.contains(&format!("\"{parameter}\"")),
            "{parameter} must not be exported: OTIO has no vocabulary for it"
        );
    }
}

#[test]
fn importing_the_export_gives_back_the_same_cut() {
    let plan = import_document(&exported()).expect("the export imports");
    let project = project();
    let sequence = project
        .sequence("Edit")
        .expect("the fixture has an Edit sequence");

    assert_eq!(plan.sequence.name, "Edit");
    assert_eq!(
        plan.sequence.frame_rate,
        sequence.settings.frame_rate.to_rate().unwrap()
    );
    // The canvas and the audio rate survive because the exporter wrote them
    // into its own metadata namespace; OTIO itself stores neither.
    assert_eq!(plan.sequence.resolution, sequence.settings.resolution);
    assert_eq!(plan.sequence.sample_rate, sequence.settings.sample_rate);

    assert_eq!(
        plan.media
            .iter()
            .map(|media| media.path.as_str())
            .collect::<Vec<_>>(),
        [
            "footage/wide.mov",
            "footage/close.mov",
            "audio/room-tone.wav"
        ]
    );

    let names: Vec<&str> = plan
        .sequence
        .tracks
        .iter()
        .map(|track| track.name.as_str())
        .collect();
    assert_eq!(names, ["V1", "A1"]);
    assert_eq!(plan.sequence.tracks[0].kind, TrackKind::Video);
    assert_eq!(plan.sequence.tracks[1].kind, TrackKind::Audio);

    let video = &plan.sequence.tracks[0].items;
    assert_eq!(video.len(), 5);
    let ImportedItem::Clip(first) = &video[0] else {
        panic!("the first item is a clip");
    };
    assert_eq!(first.name, "Wide");
    assert_eq!(first.media_index, 0);
    assert_eq!(first.source_range.start().value(), 0);
    assert_eq!(first.source_range.duration().value(), 48);
    assert_eq!(first.source_range.duration().rate().denominator(), 1001);

    let ImportedItem::Transition {
        in_offset,
        out_offset,
    } = &video[1]
    else {
        panic!("the second item is a transition");
    };
    assert_eq!((in_offset.value(), out_offset.value()), (12, 12));

    let ImportedItem::Gap(gap) = &video[3] else {
        panic!("the fourth item is a gap");
    };
    assert_eq!(gap.value(), 24);

    let ImportedItem::Clip(close) = &video[2] else {
        panic!("the third item is a clip");
    };
    assert_eq!(close.media_index, 1);
    assert_eq!(close.markers.len(), 1);
    assert_eq!(close.markers[0].name, "eyeline");
    assert_eq!(close.markers[0].note, "check the eyeline against the wide");
    assert_eq!(close.markers[0].marked_range.start().value(), 40);

    assert_eq!(plan.sequence.markers.len(), 1);
    assert_eq!(plan.sequence.markers[0].name, "act one");
    assert_eq!(plan.sequence.markers[0].marked_range.duration().value(), 48);

    // A document this plugin wrote loses nothing, so it has nothing to report.
    assert!(plan.notes.is_empty(), "{:?}", plan.notes);
}
