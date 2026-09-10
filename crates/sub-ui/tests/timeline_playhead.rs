//! The playhead on the timeline: click-to-seek, scrubbing and snapping.
//!
//! These go through the shared `egui_kittest` harness in `tests/support`, so
//! the frame is the same one every other panel test paints into and the
//! pointer is driven the way a real one is: a move, a press, a drag, a
//! release, one frame each. Nothing here asserts on pixels except the one
//! snapshot at the bottom; the rest assert on the panel's own playhead, which
//! is the model the ruler is driving.
//!
//! Positions are read back from [`TimelinePanel::layout`] after a first frame
//! rather than hard-coded, because where the panel sits inside the harness
//! frame is the harness's business, not this test's.

mod support;

use eframe::egui::{self, Pos2};
use sub_model::marker::Marker;
use sub_model::media::{StreamInfo, VideoStream};
use sub_model::sequence::SequenceSettings;
use sub_model::{
    Clip, ColorTags, Gap, MediaItem, MediaPath, Project, Sequence, Track, TrackItem, TrackKind,
};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::snapping::{SnapKind, SnapSettings};
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::TimelinePanel;
use sub_ui::viewer::ViewerState;

/// The sequence timebase every test here uses.
const RATE: Rational = Rational::FPS_24;

/// Where the first clip ends and the second one starts, in frames.
///
/// The scene is deliberately sparse so that a click far from a cut is
/// unambiguous and a click near one is unambiguously near it.
const FIRST_CUT: i64 = 48;
const SECOND_CLIP_START: i64 = 480;

fn frames(value: i64) -> RationalTime {
    RationalTime::new(value, RATE)
}

fn range(start: i64, duration: i64) -> TimeRange {
    TimeRange::new(frames(start), frames(duration)).expect("a valid range")
}

/// A project with one video track: a clip at 0..48, a gap, a clip at 480..528.
fn scene() -> (Project, Sequence) {
    let mut project = Project::new("playhead");
    let mut item = MediaItem::new(MediaPath::new("media/take.mp4").expect("a valid path"));
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
    let mut track = Track::new("V1", TrackKind::Video);
    track.items.push(TrackItem::Clip(Clip::new(
        "head",
        media,
        range(0, FIRST_CUT),
    )));
    track.items.push(TrackItem::Gap(Gap::new(frames(
        SECOND_CLIP_START - FIRST_CUT,
    ))));
    track
        .items
        .push(TrackItem::Clip(Clip::new("tail", media, range(0, 48))));
    sequence.tracks.push(track);
    project.sequences.push(sequence.clone());
    (project, sequence)
}

/// What a harness in this file carries: the panel, what it paints, and the
/// last seek it asked for.
struct Scene {
    panel: TimelinePanel,
    project: Project,
    sequence: Sequence,
    seek: Option<RationalTime>,
    /// The viewer the seek is applied to, exactly as the application does it.
    viewer: ViewerState,
    /// Whether applying a seek told the application to composite again.
    composited: bool,
}

/// A harness painting the timeline panel over [`scene`] at one pixel a frame.
fn harness<'a>() -> egui_kittest::Harness<'a, Scene> {
    let (project, sequence) = scene();
    let mut panel = TimelinePanel::new(RATE);
    panel.view_mut().set_zoom(ZoomLevel::ONE);
    let viewer = ViewerState::for_sequence(&sequence);
    let state = Scene {
        panel,
        project,
        sequence,
        seek: None,
        viewer,
        composited: false,
    };
    let mut harness = support::panel_harness_state(state, |ui, scene| {
        scene.panel.sync(&scene.sequence, 1);
        // The viewer owns the playhead, so it is handed to the panel before
        // the frame and takes back whatever the frame asked for: the same
        // round trip `SubordinateApp::dock_ui` makes.
        scene.panel.set_playhead(scene.viewer.playhead());
        let response = scene.panel.ui(ui, &scene.project, &scene.sequence);
        if let Some(time) = response.seek {
            scene.seek = Some(time);
            scene.composited |= scene.viewer.seek_to(time);
        }
    });
    harness.run();
    harness
}

/// The point on the ruler `offset` points right of the start of the lanes.
///
/// # Panics
///
/// Panics before the panel has painted once, which the harness above does.
fn ruler_at(scene: &Scene, offset: f32) -> Pos2 {
    let layout = scene.panel.layout().expect("the panel has painted");
    egui::pos2(layout.content.left() + offset, layout.ruler.center().y)
}

/// The point in the first track's lane `offset` points right of the lanes.
fn lane_at(scene: &Scene, offset: f32) -> Pos2 {
    let layout = scene.panel.layout().expect("the panel has painted");
    egui::pos2(layout.content.left() + offset, layout.content.top() + 20.0)
}

/// Moves the pointer to `pos`, presses, releases and takes the cursor away.
fn click(harness: &mut egui_kittest::Harness<'_, Scene>, pos: Pos2) {
    harness.hover_at(pos);
    harness.run();
    harness.drag_at(pos);
    harness.run();
    harness.drop_at(pos);
    harness.run();
}

#[test]
fn clicking_the_ruler_seeks_to_the_frame_under_the_pointer() {
    let mut harness = harness();
    *harness.state_mut().panel.snap_settings_mut() = SnapSettings::off();
    assert_eq!(
        harness.state().panel.playhead(),
        frames(0),
        "a fresh panel starts at the head of the sequence"
    );

    let target = ruler_at(harness.state(), 200.0);
    click(&mut harness, target);

    assert_eq!(
        harness.state().panel.playhead(),
        frames(200),
        "one pixel a frame, so 200 points along is frame 200"
    );
    assert_eq!(
        harness.state().seek,
        Some(frames(200)),
        "and the panel reported the seek for the viewer to follow"
    );

    let scene = harness.state();
    assert_eq!(
        scene.viewer.playhead(),
        frames(200),
        "the viewer followed the ruler, which is what updates the picture"
    );
    assert!(
        scene.composited,
        "and said so, so the application composites the new frame"
    );
    assert_eq!(scene.viewer.timecode_label(), "00:00:08:08");
}

#[test]
fn dragging_the_ruler_scrubs() {
    let mut harness = harness();
    *harness.state_mut().panel.snap_settings_mut() = SnapSettings::off();

    let start = ruler_at(harness.state(), 120.0);
    harness.hover_at(start);
    harness.run();
    harness.drag_at(start);
    harness.run();
    assert_eq!(harness.state().panel.playhead(), frames(120));

    for offset in [160.0_f32, 240.0, 300.0] {
        let next = ruler_at(harness.state(), offset);
        harness.hover_at(next);
        harness.run();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the offsets above are small whole numbers of points"
        )]
        let expected = frames(offset as i64);
        assert_eq!(
            harness.state().panel.playhead(),
            expected,
            "the playhead follows the pointer while the button is down"
        );
    }

    let end = ruler_at(harness.state(), 300.0);
    harness.drop_at(end);
    harness.run();
    assert_eq!(
        harness.state().panel.playhead(),
        frames(300),
        "and stays where the drag ended"
    );

    // With the button up, moving the pointer over the ruler does nothing.
    let elsewhere = ruler_at(harness.state(), 400.0);
    harness.hover_at(elsewhere);
    harness.run();
    assert_eq!(harness.state().panel.playhead(), frames(300));
}

#[test]
fn a_click_near_a_cut_snaps_onto_it() {
    let mut harness = harness();
    assert!(
        harness.state().panel.snap_settings().enabled,
        "snapping is on until the user turns it off"
    );
    let threshold = harness.state().panel.snap_settings().threshold_px;

    // Inside the threshold of the cut at frame 48: the playhead lands on the
    // cut, not on the pixel that was clicked.
    #[expect(
        clippy::cast_precision_loss,
        reason = "the threshold is a small whole number of points"
    )]
    let inside = ruler_at(harness.state(), (FIRST_CUT + 3) as f32);
    click(&mut harness, inside);
    assert_eq!(harness.state().panel.playhead(), frames(FIRST_CUT));

    // And well outside it: no pull at all.
    let outside_frames = FIRST_CUT + i64::from(threshold) + 4;
    #[expect(
        clippy::cast_precision_loss,
        reason = "the offset is a small whole number of points"
    )]
    let outside = ruler_at(harness.state(), outside_frames as f32);
    click(&mut harness, outside);
    assert_eq!(harness.state().panel.playhead(), frames(outside_frames));
}

#[test]
fn snapping_off_lands_exactly_where_the_pointer_is() {
    let mut harness = harness();
    assert!(
        !harness.state_mut().panel.toggle_snapping(),
        "the S shortcut turns snapping off"
    );

    #[expect(
        clippy::cast_precision_loss,
        reason = "the offset is a small whole number of points"
    )]
    let near_cut = ruler_at(harness.state(), (FIRST_CUT + 3) as f32);
    click(&mut harness, near_cut);
    assert_eq!(
        harness.state().panel.playhead(),
        frames(FIRST_CUT + 3),
        "with snapping off the cut has no pull"
    );

    assert!(
        harness.state_mut().panel.toggle_snapping(),
        "and S turns it back on"
    );
    click(&mut harness, near_cut);
    assert_eq!(harness.state().panel.playhead(), frames(FIRST_CUT));
}

#[test]
fn the_snap_targets_are_the_clip_edges_markers_playhead_and_sequence_start() {
    let mut harness = harness();
    harness
        .state_mut()
        .sequence
        .markers
        .push(Marker::new("cue", TimeRange::empty_at(frames(300))));
    // The markers changed, so the panel's indexes are stale by a revision.
    harness.state_mut().panel.invalidate();
    harness.run();

    let scene = harness.state_mut();
    scene.panel.set_playhead(frames(120));
    scene.panel.collect_snap_candidates(&scene.sequence, true);
    let kinds: Vec<(SnapKind, i64)> = scene
        .panel
        .snap_candidates()
        .iter()
        .map(|candidate| (candidate.kind, candidate.time.value()))
        .collect();

    for expected in [
        (SnapKind::SequenceStart, 0),
        (SnapKind::ClipEdge, 0),
        (SnapKind::ClipEdge, FIRST_CUT),
        (SnapKind::Playhead, 120),
        (SnapKind::Marker, 300),
        (SnapKind::ClipEdge, SECOND_CLIP_START),
    ] {
        assert!(
            kinds.contains(&expected),
            "{expected:?} should be a snap target, got {kinds:?}"
        );
    }
}

#[test]
fn the_snap_threshold_is_pixels_and_not_frames() {
    let mut harness = harness();
    let threshold = f32::from(
        u16::try_from(harness.state().panel.snap_settings().threshold_px).expect("a small value"),
    );

    // Zoomed in to four pixels a frame, the same eight points reach only two
    // frames, so a click three frames past the cut no longer snaps to it —
    // the reach on screen is unchanged and the reach in time has shrunk.
    harness
        .state_mut()
        .panel
        .view_mut()
        .set_zoom(ZoomLevel::clamped(
            Rational::new(4, 1).expect("a positive zoom"),
        ));
    harness.run();

    #[expect(
        clippy::cast_precision_loss,
        reason = "the cut is a small whole number of frames"
    )]
    let cut_px = FIRST_CUT as f32 * 4.0;
    let just_inside = ruler_at(harness.state(), cut_px + threshold - 1.0);
    click(&mut harness, just_inside);
    assert_eq!(
        harness.state().panel.playhead(),
        frames(FIRST_CUT),
        "seven points past the cut is inside an eight-point threshold"
    );

    let just_outside = ruler_at(harness.state(), cut_px + threshold + 4.0);
    click(&mut harness, just_outside);
    assert_ne!(
        harness.state().panel.playhead(),
        frames(FIRST_CUT),
        "twelve points past it is outside, however many frames that is"
    );
}

#[test]
fn a_click_in_a_lane_does_not_move_the_playhead() {
    let mut harness = harness();
    *harness.state_mut().panel.snap_settings_mut() = SnapSettings::off();
    let target = ruler_at(harness.state(), 200.0);
    click(&mut harness, target);
    assert_eq!(harness.state().panel.playhead(), frames(200));

    // Seeking is the ruler's gesture; a press in a lane belongs to clip
    // selection (TASK-30) and must leave the playhead alone.
    let lane = lane_at(harness.state(), 320.0);
    click(&mut harness, lane);
    assert_eq!(
        harness.state().panel.playhead(),
        frames(200),
        "the lane click was not a seek"
    );
}

#[test]
fn the_playhead_never_goes_before_the_head_of_the_sequence() {
    let mut harness = harness();
    *harness.state_mut().panel.snap_settings_mut() = SnapSettings::off();
    harness.state_mut().panel.set_playhead(frames(200));

    // The very start of the ruler is the head of the sequence, and there is
    // no sequence time before it to click on.
    let start = ruler_at(harness.state(), 0.0);
    click(&mut harness, start);
    assert_eq!(harness.state().panel.playhead(), frames(0));

    // Nor can one be set: a negative time clamps rather than painting the
    // playhead off the left of the panel.
    harness.state_mut().panel.set_playhead(frames(-30));
    assert_eq!(harness.state().panel.playhead(), frames(0));
}

#[test]
fn the_timeline_panel_with_a_playhead_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project).clone();
    let rate = sequence.settings.frame_rate;
    let mut panel = TimelinePanel::new(rate);
    let mut harness = support::panel_harness(|ui| {
        panel.sync(&sequence, 1);
        // Far enough along that the playhead is unmistakably drawn over the
        // lanes rather than sitting on the left edge of the panel.
        panel.set_playhead(RationalTime::new(180, rate));
        panel.ui(ui, &project, &sequence);
    });
    harness.run();
    support::snapshot(&mut harness, "timeline_playhead");
}
