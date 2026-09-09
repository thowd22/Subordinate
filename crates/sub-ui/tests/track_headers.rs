//! Driving the track header controls headlessly, and applying what they raise.
//!
//! `egui::Context::run_ui` runs a whole frame — input, layout, interaction and
//! painting — with no window and no GPU, so a click on the mute toggle can be
//! synthesised here and the action it raises can be applied through the
//! Command API and undone, which is the whole contract of the header column.

use eframe::egui::{self, Pos2, Rect, Vec2};
use sub_edit::commands::SetClipParams;
use sub_edit::{Command, History, codes};
use sub_model::params::Opacity;
use sub_model::sequence::SequenceSettings;
use sub_model::{Clip, MediaItem, MediaPath, Project, Sequence, Track, TrackItem, TrackKind};
use sub_model::{ColorTags, media::StreamInfo, media::VideoStream};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::timeline_panel::TimelinePanel;
use sub_ui::track_header::{HeaderLayout, TrackAction};
use sub_ui::{ClipMediaKind, clip_edits_allowed};

/// The sequence timebase every test here uses.
const RATE: Rational = Rational::FPS_24;

/// A project with `tracks` video tracks, each holding one clip.
fn scene(tracks: usize) -> (Project, Sequence) {
    let mut project = Project::new("headers");
    let mut item = MediaItem::new(MediaPath::new("media/take.mp4").expect("valid path"));
    item.info = Some(StreamInfo {
        duration: Some(RationalTime::new(10_000, RATE)),
        video: vec![VideoStream {
            width: 1920,
            height: 1080,
            frame_rate: RATE,
            sample_aspect: Rational::ONE,
            color: ColorTags::REC709,
        }],
        audio: Vec::new(),
    });
    let media = item.id;
    project.media.push(item);

    let mut sequence = Sequence::new("edit", SequenceSettings::default());
    for index in 0..tracks {
        let mut track = Track::new(format!("V{}", index + 1), TrackKind::Video);
        let source = TimeRange::new(RationalTime::new(0, RATE), RationalTime::new(48, RATE))
            .expect("valid source range");
        track
            .items
            .push(TrackItem::Clip(Clip::new("take", media, source)));
        sequence.tracks.push(track);
    }
    project.sequences.push(sequence.clone());
    (project, sequence)
}

/// Runs one frame, feeding it `events`, and returns what the headers raised
/// plus the shapes the frame painted.
fn frame(
    ctx: &egui::Context,
    panel: &mut TimelinePanel,
    project: &Project,
    sequence: &Sequence,
    revision: u64,
    events: Vec<egui::Event>,
) -> (Vec<TrackAction>, Vec<egui::epaint::ClippedShape>) {
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 600.0))),
        events,
        ..Default::default()
    };
    let mut raised = Vec::new();
    let mut output = ctx.run_ui(input, |ui| {
        panel.sync(sequence, revision);
        raised = panel.ui(ui, project, sequence).actions;
    });
    let shapes = std::mem::take(&mut output.shapes);
    output.textures_delta.clear();
    (raised, shapes)
}

/// The events that move the pointer to `pos` and click there.
fn click_at(pos: Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        },
    ]
}

/// The events that right-click at `pos`.
fn right_click_at(pos: Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        },
    ]
}

/// Every piece of text in `shapes`, however deeply nested.
fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, into);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut found);
    }
    found
}

/// Where the text `wanted` was painted, if it was.
fn text_center(shapes: &[egui::epaint::ClippedShape], wanted: &str) -> Option<Pos2> {
    fn walk(shape: &egui::Shape, wanted: &str, into: &mut Option<Pos2>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == wanted => {
                *into = Some(text.pos + text.galley.size() / 2.0);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, wanted, into);
                }
            }
            _ => {}
        }
    }
    let mut found = None;
    for clipped in shapes {
        walk(&clipped.shape, wanted, &mut found);
    }
    found
}

/// Every rectangle fill colour in `shapes`.
fn fills(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
    shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => Some(rect.fill),
            _ => None,
        })
        .collect()
}

#[test]
fn clicking_the_mute_and_lock_toggles_raises_undoable_actions() {
    let (mut project, sequence) = scene(2);
    let sequence_id = sequence.id;
    let track_id = sequence.tracks[0].id;
    let mut panel = TimelinePanel::new(RATE);
    let ctx = egui::Context::default();

    // One frame to lay the headers out, so the next frame's click has
    // something to hit.
    frame(&ctx, &mut panel, &project, &sequence, 1, Vec::new());
    let header = panel.header_rect(0).expect("the panel has been painted");
    let controls = HeaderLayout::new(header);

    for (target, expected) in [
        (
            controls.mute.center(),
            TrackAction::SetMuted {
                track: track_id,
                muted: true,
            },
        ),
        (
            controls.lock.center(),
            TrackAction::SetLocked {
                track: track_id,
                locked: true,
            },
        ),
    ] {
        // Hover first: egui hit-tests against the previous frame's widget
        // rectangles, exactly as a real pointer does.
        frame(
            &ctx,
            &mut panel,
            &project,
            &sequence,
            1,
            vec![egui::Event::PointerMoved(target)],
        );
        let (actions, _) = frame(&ctx, &mut panel, &project, &sequence, 1, click_at(target));
        assert_eq!(actions, vec![expected.clone()], "clicking at {target:?}");

        let mut history = History::new();
        history
            .apply_boxed(&mut project, expected.clone().into_command(sequence_id))
            .expect("the action applies");
        let track = &project.sequences[0].tracks[0];
        match expected {
            TrackAction::SetMuted { .. } => assert!(track.muted),
            TrackAction::SetLocked { .. } => assert!(track.locked),
            other => panic!("unexpected action {other:?}"),
        }
        history.undo(&mut project).expect("undo");
        let track = &project.sequences[0].tracks[0];
        assert!(!track.muted && !track.locked, "the action undoes");
    }
}

#[test]
fn a_locked_track_paints_its_clips_dimmed() {
    let (project, mut sequence) = scene(1);
    let mut panel = TimelinePanel::new(RATE);
    let ctx = egui::Context::default();

    let (_, open) = frame(&ctx, &mut panel, &project, &sequence, 1, Vec::new());
    let bright = ClipMediaKind::Video.fill();
    assert!(
        fills(&open).contains(&bright),
        "an unlocked track paints its clips at full strength"
    );

    sequence.tracks[0].locked = true;
    let (_, locked) = frame(&ctx, &mut panel, &project, &sequence, 2, Vec::new());
    let painted = fills(&locked);
    assert!(
        !painted.contains(&bright),
        "a locked track must not paint a clip at full strength"
    );
    assert!(
        painted
            .iter()
            .any(|fill| *fill != bright && fill.a() > 0 && *fill == dimmed(bright)),
        "the clip is painted dimmed instead: {painted:?}"
    );
}

/// The dimmed form of `color`, as the panel paints a locked track's clips.
fn dimmed(color: egui::Color32) -> egui::Color32 {
    color.gamma_multiply(0.45)
}

#[test]
fn a_locked_track_refuses_clip_edits() {
    let (mut project, sequence) = scene(1);
    let track = &project.sequences[0].tracks[0];
    assert!(clip_edits_allowed(track), "a new track is open to edits");

    project.sequences[0].tracks[0].locked = true;
    let locked = &project.sequences[0].tracks[0];
    assert!(
        !clip_edits_allowed(locked),
        "the panel treats a locked track as closed to clip edits"
    );
    let clip = locked.clips().next().expect("a clip").id;
    let track_id = locked.id;

    // A clip edit routed through the lock lookup is refused, which is what
    // the dimmed lane is telling the editor before they try.
    let error = SetClipParams::new(sequence.id, track_id, clip)
        .with_opacity(Opacity::TRANSPARENT)
        .apply(&mut project)
        .expect_err("a clip edit on a locked track is refused");
    assert_eq!(error.code, codes::TRACK_LOCKED);
}

#[test]
fn the_headers_scroll_with_the_lanes() {
    let (project, sequence) = scene(20);
    let mut panel = TimelinePanel::new(RATE);
    let ctx = egui::Context::default();
    frame(&ctx, &mut panel, &project, &sequence, 1, Vec::new());

    let before = panel.header_rect(0).expect("painted");
    panel.set_lane_scroll(120.0, 400.0, 20);
    frame(&ctx, &mut panel, &project, &sequence, 1, Vec::new());
    let after = panel.header_rect(0).expect("painted");
    assert!(
        after.top() < before.top(),
        "scrolling the lanes moves the headers with them"
    );
    assert_eq!(after.size(), before.size(), "the header keeps its size");
    assert!(
        (after.width() - Vec2::new(panel.metrics().header_width, 0.0).x).abs() < 0.01,
        "the header column keeps its width"
    );
}

#[test]
fn a_header_paints_its_name_and_its_toggles() {
    let (project, sequence) = scene(1);
    let mut panel = TimelinePanel::new(RATE);
    let ctx = egui::Context::default();
    let (_, shapes) = frame(&ctx, &mut panel, &project, &sequence, 1, Vec::new());
    let painted = texts(&shapes);
    for expected in ["V1", "video", "M", "L"] {
        assert!(
            painted.iter().any(|text| text == expected),
            "the header should show {expected:?}: {painted:?}"
        );
    }
}

#[test]
fn right_clicking_a_header_opens_the_track_menu() {
    let (project, sequence) = scene(2);
    let mut panel = TimelinePanel::new(RATE);
    let ctx = egui::Context::default();
    frame(&ctx, &mut panel, &project, &sequence, 1, Vec::new());
    let header = panel.header_rect(0).expect("the panel has been painted");
    let target = header.center();

    frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        vec![egui::Event::PointerMoved(target)],
    );
    frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        right_click_at(target),
    );
    // The popup is laid out on the frame after the one that opened it.
    let (_, shapes) = frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        vec![egui::Event::PointerMoved(target)],
    );
    let painted = texts(&shapes);
    for expected in [
        "Add video track above",
        "Add audio track above",
        "Rename track",
        "Move up",
        "Remove track and its clips",
    ] {
        assert!(
            painted.iter().any(|text| text == expected),
            "the menu should offer {expected:?}: {painted:?}"
        );
    }
}

#[test]
fn choosing_move_up_from_the_menu_raises_an_undoable_reorder() {
    let (mut project, sequence) = scene(2);
    let sequence_id = sequence.id;
    let first = sequence.tracks[0].id;
    let mut panel = TimelinePanel::new(RATE);
    let ctx = egui::Context::default();
    frame(&ctx, &mut panel, &project, &sequence, 1, Vec::new());
    let target = panel.header_rect(0).expect("painted").center();

    frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        vec![egui::Event::PointerMoved(target)],
    );
    frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        right_click_at(target),
    );
    let (_, shapes) = frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        vec![egui::Event::PointerMoved(target)],
    );
    let entry = text_center(&shapes, "Move up").expect("the menu offers Move up");

    frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        vec![egui::Event::PointerMoved(entry)],
    );
    let (actions, _) = frame(&ctx, &mut panel, &project, &sequence, 1, click_at(entry));
    assert_eq!(
        actions,
        vec![TrackAction::Reorder {
            track: first,
            to_index: 1
        }]
    );

    let mut history = History::new();
    history
        .apply_boxed(&mut project, actions[0].clone().into_command(sequence_id))
        .expect("the reorder applies");
    assert_eq!(project.sequences[0].tracks[1].id, first, "the track moved");
    history.undo(&mut project).expect("undo");
    assert_eq!(project.sequences[0].tracks[0].id, first, "and moved back");
}

#[test]
fn renaming_from_the_menu_types_a_new_name_and_commits_it() {
    let (mut project, sequence) = scene(1);
    let sequence_id = sequence.id;
    let track = sequence.tracks[0].id;
    let mut panel = TimelinePanel::new(RATE);
    let ctx = egui::Context::default();
    frame(&ctx, &mut panel, &project, &sequence, 1, Vec::new());
    let target = panel.header_rect(0).expect("painted").center();

    frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        vec![egui::Event::PointerMoved(target)],
    );
    frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        right_click_at(target),
    );
    let (_, shapes) = frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        vec![egui::Event::PointerMoved(target)],
    );
    let entry = text_center(&shapes, "Rename track").expect("the menu offers a rename");
    frame(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        1,
        vec![egui::Event::PointerMoved(entry)],
    );
    let (actions, _) = frame(&ctx, &mut panel, &project, &sequence, 1, click_at(entry));
    assert!(actions.is_empty(), "opening the editor is not an edit");

    // The editor is open over the name; type into it and press Enter.
    frame(&ctx, &mut panel, &project, &sequence, 1, Vec::new());
    let typed = vec![
        egui::Event::Text("Dialogue".to_owned()),
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        },
    ];
    let (actions, _) = frame(&ctx, &mut panel, &project, &sequence, 1, typed);
    assert_eq!(
        actions,
        vec![TrackAction::Rename {
            track,
            name: "Dialogue".to_owned()
        }]
    );

    let mut history = History::new();
    history
        .apply_boxed(&mut project, actions[0].clone().into_command(sequence_id))
        .expect("the rename applies");
    assert_eq!(project.sequences[0].tracks[0].name, "Dialogue");
    history.undo(&mut project).expect("undo");
    assert_eq!(project.sequences[0].tracks[0].name, "V1");
}
