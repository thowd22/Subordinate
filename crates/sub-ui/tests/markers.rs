//! Markers on the timeline ruler: dropping, dragging, renaming and removing
//! one, and what each of those asks the Command API to do.
//!
//! These go through the shared `egui_kittest` harness in `tests/support`, so
//! the frame is the one every other panel test paints into and the pointer is
//! driven the way a real one is. Nothing here asserts on pixels except the one
//! snapshot at the bottom; the rest drive a gesture, take the
//! [`MarkerAction`](sub_ui::markers::MarkerAction) the panel raised, apply it
//! through a real [`History`] and assert on the project — which is the whole
//! point of the action being a command rather than an edit the panel made
//! itself.

mod support;

use eframe::egui::{self, Pos2};
use sub_edit::History;
use sub_model::marker::Marker;
use sub_model::media::{StreamInfo, VideoStream};
use sub_model::sequence::SequenceSettings;
use sub_model::{
    Clip, ColorTags, Gap, MarkerId, MediaItem, MediaPath, Project, Sequence, Track, TrackItem,
    TrackKind,
};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::markers::{DEFAULT_MARKER_NAME, MARKER_PALETTE, MarkerAction, marker_color};
use sub_ui::snapping::SnapSettings;
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::TimelinePanel;

/// The sequence timebase every test here uses.
const RATE: Rational = Rational::FPS_24;

/// Where the first clip ends, in frames: a snap target for a dragged marker.
const FIRST_CUT: i64 = 48;

/// Where the fixture's one marker starts, in frames.
const MARKER_AT: i64 = 200;

fn frames(value: i64) -> RationalTime {
    RationalTime::new(value, RATE)
}

fn range(start: i64, duration: i64) -> TimeRange {
    TimeRange::new(frames(start), frames(duration)).expect("a valid range")
}

/// A project with one video track and one point marker at [`MARKER_AT`].
fn scene() -> (Project, Sequence, MarkerId) {
    let mut project = Project::new("markers");
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
    track.items.push(TrackItem::Gap(Gap::new(frames(432))));
    track
        .items
        .push(TrackItem::Clip(Clip::new("tail", media, range(0, 48))));
    sequence.tracks.push(track);

    let marker = Marker::new("cue", TimeRange::empty_at(frames(MARKER_AT)));
    let marker_id = marker.id;
    sequence.markers.push(marker);
    project.sequences.push(sequence.clone());
    (project, sequence, marker_id)
}

/// What a harness in this file carries.
///
/// The project and the sequence are kept in step the way the application
/// keeps them: a command is applied to the project through the history, and
/// the sequence the panel paints is refreshed from it.
struct Scene {
    panel: TimelinePanel,
    project: Project,
    sequence: Sequence,
    history: History,
    /// Every marker action the panel has raised, in order.
    raised: Vec<MarkerAction>,
}

impl Scene {
    /// The sequence as the project now has it.
    fn refresh(&mut self) {
        self.sequence = self.project.sequences[0].clone();
        self.panel.invalidate();
    }

    /// The marker with `id`, as the project now has it.
    fn marker(&self, id: MarkerId) -> Option<&Marker> {
        self.project.sequences[0].marker(id)
    }

    /// Undoes the last command and shows the project as it now stands.
    ///
    /// Every marker gesture is one command, so this is the whole of what
    /// Ctrl+Z does to one.
    fn undo(&mut self) {
        self.history
            .undo(&mut self.project)
            .expect("the last marker command is undoable");
        self.refresh();
    }
}

/// A harness painting the timeline panel over [`scene`] at one pixel a frame.
///
/// Marker actions are applied as commands as they arrive, exactly as the
/// application will: the panel raised them, the history applied them, and
/// undo is therefore whatever the command set says it is.
fn harness<'a>() -> egui_kittest::Harness<'a, Scene> {
    let (project, sequence, _) = scene();
    let mut panel = TimelinePanel::new(RATE);
    panel.view_mut().set_zoom(ZoomLevel::ONE);
    let state = Scene {
        panel,
        project,
        sequence,
        history: History::new(),
        raised: Vec::new(),
    };
    // A sixtieth of a second a frame rather than the harness default of a
    // quarter: a double-click has to land inside egui's double-click window,
    // and at four frames a second two clicks are half a second apart.
    let mut harness = support::builder().with_step_dt(1.0 / 60.0).build_ui_state(
        |ui: &mut egui::Ui, scene: &mut Scene| {
            scene.panel.sync(&scene.sequence, 1);
            let response = scene.panel.ui(ui, &scene.project, &scene.sequence);
            let mut applied = false;
            for action in response.marker_actions {
                scene.raised.push(action.clone());
                let sequence_id = scene.sequence.id;
                scene
                    .history
                    .apply_boxed(&mut scene.project, action.into_command(sequence_id))
                    .expect("a marker command the panel raised applies");
                applied = true;
            }
            if applied {
                scene.refresh();
            }
        },
        state,
    );
    harness.run();
    harness
}

/// The point on the ruler `offset` points right of the start of the lanes.
fn ruler_at(scene: &Scene, offset: f32) -> Pos2 {
    let layout = scene.panel.layout().expect("the panel has painted");
    egui::pos2(layout.content.left() + offset, layout.ruler.center().y)
}

/// The middle of the flag of the marker whose head sits at `frame`.
fn flag_at(scene: &Scene, frame: i64) -> Pos2 {
    scene
        .panel
        .marker_flag_rect(frames(frame))
        .expect("the panel has painted")
        .center()
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

/// Presses on `from`, drags to `to` and releases there.
fn drag(harness: &mut egui_kittest::Harness<'_, Scene>, from: Pos2, to: Pos2) {
    harness.hover_at(from);
    harness.run();
    harness.drag_at(from);
    harness.run();
    harness.hover_at(to);
    harness.run();
    harness.drop_at(to);
    harness.run();
}

#[test]
fn m_drops_a_marker_at_the_playhead_and_undo_takes_it_away() {
    let mut harness = harness();
    harness.state_mut().panel.set_playhead(frames(120));
    let id = harness.state_mut().panel.add_marker_at_playhead();
    harness.run();

    let scene = harness.state();
    assert_eq!(
        scene.raised.len(),
        1,
        "the shortcut raised exactly one action"
    );
    let marker = scene.marker(id).expect("the marker reached the project");
    assert_eq!(marker.marked_range.start(), frames(120));
    assert!(marker.is_point(), "M drops a point, not a span");
    assert_eq!(marker.name, DEFAULT_MARKER_NAME);

    harness.state_mut().undo();
    harness.run();
    assert!(harness.state().marker(id).is_none(), "undo took it away");
}

#[test]
fn dragging_a_marker_moves_it_and_the_move_is_undoable() {
    let mut harness = harness();
    *harness.state_mut().panel.snap_settings_mut() = SnapSettings::off();
    let id = harness.state().sequence.markers[0].id;

    let from = flag_at(harness.state(), MARKER_AT);
    let to = egui::pos2(from.x + 100.0, from.y);
    drag(&mut harness, from, to);

    let scene = harness.state();
    assert_eq!(
        scene.raised.len(),
        1,
        "one drag is one move, raised on the release: {:?}",
        scene.raised
    );
    assert!(
        matches!(scene.raised[0], MarkerAction::Move { marker, .. } if marker == id),
        "the drag moved the marker it grabbed: {:?}",
        scene.raised[0]
    );
    assert_eq!(
        scene.marker(id).expect("still there").marked_range.start(),
        frames(MARKER_AT + 100),
        "a hundred points at one pixel a frame is a hundred frames"
    );

    harness.state_mut().undo();
    assert_eq!(
        harness
            .state()
            .marker(id)
            .expect("still there")
            .marked_range
            .start(),
        frames(MARKER_AT),
        "undo put it back on the frame it came from"
    );
}

#[test]
fn a_dragged_marker_snaps_onto_a_cut() {
    let mut harness = harness();
    let id = harness.state().sequence.markers[0].id;
    assert!(
        harness.state().panel.snap_settings().enabled,
        "snapping is on until the user turns it off"
    );

    // Dropped three points past the cut at frame 48, which is inside the
    // threshold: the marker lands on the cut rather than beside it.
    let from = flag_at(harness.state(), MARKER_AT);
    #[expect(
        clippy::cast_precision_loss,
        reason = "the cut is a small whole number of frames"
    )]
    let to = ruler_at(harness.state(), (FIRST_CUT + 3) as f32);
    drag(&mut harness, from, to);

    assert_eq!(
        harness
            .state()
            .marker(id)
            .expect("still there")
            .marked_range
            .start(),
        frames(FIRST_CUT),
        "markers are dropped on snap targets like everything else"
    );
}

#[test]
fn a_marker_is_a_snap_target_for_the_playhead() {
    let mut harness = harness();
    let threshold = f32::from(
        u16::try_from(harness.state().panel.snap_settings().threshold_px).expect("a small value"),
    );

    // A click on the ruler just short of the marker lands on the marker.
    #[expect(
        clippy::cast_precision_loss,
        reason = "the marker sits on a small whole number of frames"
    )]
    let near = ruler_at(harness.state(), MARKER_AT as f32 - threshold + 1.0);
    click(&mut harness, near);
    assert_eq!(
        harness.state().panel.playhead(),
        frames(MARKER_AT),
        "the playhead was pulled onto the marker"
    );
    assert!(
        harness.state().raised.is_empty(),
        "and seeking is not an edit"
    );
}

#[test]
fn double_clicking_a_marker_renames_it_and_the_rename_is_undoable() {
    let mut harness = harness();
    let id = harness.state().sequence.markers[0].id;
    let flag = flag_at(harness.state(), MARKER_AT);

    // Two clicks in quick succession over the flag: the editor opens seeded
    // with the old name, whole and selected.
    harness.hover_at(flag);
    harness.run();
    harness.drag_at(flag);
    harness.run();
    harness.drop_at(flag);
    harness.run();
    harness.hover_at(flag);
    harness.drag_at(flag);
    harness.run();
    harness.drop_at(flag);
    harness.run();
    assert_eq!(
        harness.state().panel.marker_state().renaming(),
        Some(id),
        "a double-click opened the rename editor"
    );

    harness
        .input_mut()
        .events
        .push(egui::Event::Text("reshoot".to_owned()));
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();

    let scene = harness.state();
    assert_eq!(scene.panel.marker_state().renaming(), None, "and closed it");
    assert!(
        matches!(scene.raised.last(), Some(MarkerAction::Rename { marker, .. }) if *marker == id),
        "the commit raised a rename: {:?}",
        scene.raised
    );
    assert_eq!(
        scene.marker(id).expect("still there").name,
        "reshoot",
        "typing over the seeded name replaced it"
    );

    harness.state_mut().undo();
    assert_eq!(
        harness.state().marker(id).expect("still there").name,
        "cue",
        "undo put the old name back on the same marker"
    );
}

#[test]
fn delete_removes_the_selected_marker_and_undo_restores_it() {
    let mut harness = harness();
    let id = harness.state().sequence.markers[0].id;

    // Clicking the flag selects the marker without moving it.
    let flag = flag_at(harness.state(), MARKER_AT);
    click(&mut harness, flag);
    assert_eq!(harness.state().panel.marker_state().selected(), Some(id));
    assert!(
        harness.state().raised.is_empty(),
        "a click that goes nowhere is not a move"
    );

    harness.key_press(egui::Key::Delete);
    harness.run();
    assert!(harness.state().marker(id).is_none(), "Delete removed it");
    assert_eq!(
        harness.state().panel.marker_state().selected(),
        None,
        "and nothing is selected any more"
    );

    harness.state_mut().undo();
    let restored = harness.state().marker(id).expect("undo brought it back");
    assert_eq!(restored.name, "cue");
    assert_eq!(restored.marked_range.start(), frames(MARKER_AT));
}

#[test]
fn a_press_on_the_ruler_away_from_a_flag_still_scrubs() {
    let mut harness = harness();
    *harness.state_mut().panel.snap_settings_mut() = SnapSettings::off();
    let target = ruler_at(harness.state(), 400.0);
    click(&mut harness, target);
    assert_eq!(
        harness.state().panel.playhead(),
        frames(400),
        "grabbing markers must not cost the ruler its scrub"
    );
    assert!(harness.state().raised.is_empty());
    assert_eq!(harness.state().panel.marker_state().selected(), None);
}

#[test]
fn markers_are_coloured_from_their_own_identifiers() {
    let (_, sequence, id) = scene();
    let color = marker_color(id);
    assert!(MARKER_PALETTE.contains(&color));
    assert_eq!(
        marker_color(sequence.markers[0].id),
        color,
        "the colour follows the marker through save, load and undo"
    );
}

#[test]
fn the_ruler_and_lanes_with_markers_match_their_snapshot() {
    if !support::can_render() {
        return;
    }
    let mut project = support::fixture_project();
    let mut sequence = support::fixture_sequence(&project).clone();
    let rate = sequence.settings.frame_rate;
    // Fixed identifiers so the swatches, which are drawn from the identifier,
    // are the same on every run and the snapshot is stable.
    for (index, (name, start, duration)) in [
        ("intro", 24_i64, 0_i64),
        ("reshoot", 96, 48),
        ("check audio", 240, 0),
    ]
    .into_iter()
    .enumerate()
    {
        let range = TimeRange::new(
            RationalTime::new(start, rate),
            RationalTime::new(duration, rate),
        )
        .expect("a valid range");
        let mut marker = Marker::new(name, range);
        marker.id = fixed_id(index);
        sequence.markers.push(marker);
    }
    project.sequences[0] = sequence.clone();

    let mut panel = TimelinePanel::new(rate);
    let mut harness = support::panel_harness(|ui| {
        panel.sync(&sequence, 1);
        panel.set_playhead(RationalTime::new(180, rate));
        panel.ui(ui, &project, &sequence);
    });
    harness.run();
    support::snapshot(&mut harness, "timeline_markers");
}

/// A marker identifier that is the same on every run, so the swatches the
/// snapshot is compared against never move.
///
/// The identifier's canonical string is its wire form, and parsing one is the
/// supported way in; only the last byte matters, because that is what
/// [`marker_color`] selects a swatch with.
fn fixed_id(index: usize) -> MarkerId {
    MarkerId::parse(&format!("0192c4d0-0000-7000-8000-00000000{index:04x}"))
        .expect("a canonical UUID")
}
