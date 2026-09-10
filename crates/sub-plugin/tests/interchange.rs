//! An importer's result reaches the project only as Command API calls.
//!
//! [`sub_plugin::import_plan`] turns what an `importer` plugin describes into
//! JSON-RPC calls; this feeds those calls to the real dispatcher over a real
//! engine, which is exactly what the host will do (TASK-84), and checks that
//! the media lands in the bin, the sequence lands in the project with its
//! timebase intact, and one `edit.undo` per call takes it all back out.

use serde_json::json;
use sub_command::{Dispatcher, Request, dispatch::EDIT_UNDO};
use sub_edit::Engine;
use sub_model::{Project, TrackItem};
use sub_plugin::{
    ImportTarget, WitClipSpec, WitImportSpec, WitMediaRef, WitMediaSpec, WitResolution,
    WitSequenceSpec, WitTrackItemSpec, WitTrackKind, WitTrackSpec, export_presets, import_plan,
};
use sub_plugin::{WitPresetDesc, WitPresetSetting, WitRational, WitRationalTime, WitTimeRange};
use sub_time::{Rational, RationalTime};

/// 23.976 as a plugin sends it: exact, and not a whole number of ticks.
fn rate() -> WitRational {
    WitRational {
        numerator: 24_000,
        denominator: 1_001,
    }
}

/// `frames` at [`rate`].
fn time(frames: i64) -> WitRationalTime {
    WitRationalTime {
        value: frames,
        rate: rate(),
    }
}

/// The half-open span `[start, start + duration)`, in frames.
fn range(start: i64, duration: i64) -> WitTimeRange {
    WitTimeRange {
        start: time(start),
        duration: time(duration),
    }
}

/// What a two-file, one-sequence import looks like coming out of a plugin.
fn specs() -> Vec<WitImportSpec> {
    vec![
        WitImportSpec::Media(WitMediaSpec {
            path: "footage/a.mov".to_owned(),
            name: String::new(),
        }),
        WitImportSpec::Media(WitMediaSpec {
            path: "footage/b.mov".to_owned(),
            name: "B roll".to_owned(),
        }),
        WitImportSpec::Sequence(WitSequenceSpec {
            name: "Reel 1".to_owned(),
            frame_rate: rate(),
            resolution: WitResolution {
                width: 1920,
                height: 1080,
            },
            sample_rate: 48_000,
            tracks: vec![WitTrackSpec {
                name: "V1".to_owned(),
                kind: WitTrackKind::Video,
                items: vec![
                    WitTrackItemSpec::Clip(WitClipSpec {
                        name: String::new(),
                        media: WitMediaRef::Imported(0),
                        source_range: range(0, 24),
                        markers: Vec::new(),
                    }),
                    WitTrackItemSpec::Gap(time(12)),
                    WitTrackItemSpec::Clip(WitClipSpec {
                        name: "Cutaway".to_owned(),
                        media: WitMediaRef::Imported(1),
                        source_range: range(48, 36),
                        markers: Vec::new(),
                    }),
                ],
            }],
            markers: Vec::new(),
        }),
    ]
}

#[test]
fn an_import_reaches_the_project_through_the_command_api_and_undoes() {
    let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
    let dispatcher = Dispatcher::new(engine.handle().clone());

    let existing = engine.handle().snapshot().sequences.len();
    let calls = import_plan(&specs(), ImportTarget::appending(existing)).unwrap();
    assert_eq!(calls.len(), 3);

    for (id, call) in calls.iter().enumerate() {
        let response = dispatcher.call(&Request::new(
            i64::try_from(id).unwrap(),
            call.method,
            Some(call.params.clone()),
        ));
        assert!(
            response.error_ref().is_none(),
            "{} was rejected: {:?}",
            call.method,
            response.error_ref(),
        );
    }

    let project = engine.handle().snapshot();
    // Both files are filed in the root bin, the unnamed one after its file.
    assert_eq!(project.media.len(), 2);
    assert_eq!(project.media[0].name, "a.mov");
    assert_eq!(project.media[1].name, "B roll");
    assert_eq!(project.root_bin.media.len(), 2);

    // The sequence arrived whole, at the exact rate the plugin described.
    let sequence = &project.sequences[existing];
    assert_eq!(sequence.name, "Reel 1");
    assert_eq!(sequence.settings.frame_rate, Rational::FPS_23_976);
    assert_eq!(sequence.settings.sample_rate, 48_000);

    let track = &sequence.tracks[0];
    assert_eq!(track.items.len(), 3);
    let TrackItem::Clip(first) = &track.items[0] else {
        panic!("the first item is a clip");
    };
    // An unnamed clip takes the name of the media it plays, and plays the
    // media the same import created.
    assert_eq!(first.name, "a.mov");
    assert_eq!(first.media, project.media[0].id);
    assert_eq!(
        first.source_range.duration(),
        RationalTime::from_frames(24, Rational::FPS_23_976),
    );
    let TrackItem::Gap(gap) = &track.items[1] else {
        panic!("the second item is a gap");
    };
    assert_eq!(
        gap.duration,
        RationalTime::from_frames(12, Rational::FPS_23_976),
    );

    // Every call was one undoable step, so the import backs out cleanly.
    for id in 0..calls.len() {
        let response = dispatcher.call(&Request::new(
            100 + i64::try_from(id).unwrap(),
            EDIT_UNDO,
            Some(json!({})),
        ));
        assert!(response.error_ref().is_none(), "undo was rejected");
    }
    let project = engine.handle().snapshot();
    assert!(project.media.is_empty());
    assert_eq!(project.sequences.len(), existing);
}

#[test]
fn a_spec_the_model_cannot_hold_never_reaches_the_dispatcher() {
    let mut specs = specs();
    // A clip pointing past the end of the import.
    specs[2] = WitImportSpec::Sequence(WitSequenceSpec {
        name: "Broken".to_owned(),
        frame_rate: rate(),
        resolution: WitResolution {
            width: 1920,
            height: 1080,
        },
        sample_rate: 48_000,
        tracks: vec![WitTrackSpec {
            name: "V1".to_owned(),
            kind: WitTrackKind::Video,
            items: vec![WitTrackItemSpec::Clip(WitClipSpec {
                name: "X".to_owned(),
                media: WitMediaRef::Imported(7),
                source_range: range(0, 24),
                markers: Vec::new(),
            })],
        }],
        markers: Vec::new(),
    });

    let error = import_plan(&specs, ImportTarget::default()).unwrap_err();
    assert_eq!(error.code.as_str(), "plugin.invalid_spec");
}

#[test]
fn exporter_presets_arrive_ready_for_the_export_panel() {
    let presets = export_presets(&[
        WitPresetDesc {
            id: "youtube.1080p".to_owned(),
            name: "YouTube 1080p".to_owned(),
            description: "H.264 in MP4".to_owned(),
            container: "mp4".to_owned(),
            frame_rate: None,
            settings: vec![WitPresetSetting {
                key: "bitrate".to_owned(),
                value: "12000000".to_owned(),
            }],
        },
        WitPresetDesc {
            id: "archive.prores".to_owned(),
            name: "ProRes 422 HQ".to_owned(),
            description: String::new(),
            container: "mov".to_owned(),
            frame_rate: Some(rate()),
            settings: Vec::new(),
        },
    ])
    .unwrap();

    // The panel lists them in the order the plugin gave, with exact rates.
    assert_eq!(
        presets.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["youtube.1080p", "archive.prores"],
    );
    assert_eq!(presets[0].frame_rate, None);
    assert_eq!(presets[1].frame_rate, Some(Rational::FPS_23_976));
    assert_eq!(presets[0].settings["bitrate"], json!(12_000_000));
}
