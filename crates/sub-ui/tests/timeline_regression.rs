//! The timeline panel's regression suite: three pictures and four gestures.
//!
//! The timeline is the most complex custom-painted surface in the editor, so
//! it gets a suite of its own (TASK-120). Two halves, both over the committed
//! sample project so a change to the fixture shows up here rather than
//! nowhere:
//!
//! - Snapshots at the three zoom levels a user actually sits at: an empty
//!   sequence, the whole cut fitted to the window, and a single frame filling
//!   the viewport around the playhead. These are the pictures that catch a
//!   moved separator or a wrong clip colour.
//! - Interactions driven the way a user drives them — the pointer for the
//!   click and the drag, real key events through the shipped
//!   [`ShortcutMap`] for `Ctrl+K` and `S` — asserting on the project after
//!   each planned edit is applied through the Command API, never on pixels.
//!
//! The keyboard half deliberately goes through [`ShortcutMap::poll`] and the
//! same dispatch `SubordinateApp::apply_shortcuts` makes, so a rebinding or a
//! dropped `else if` arm fails here and not only in the running editor.

mod support;

use std::cell::Cell;

use eframe::egui::{self, Key, Modifiers, Pos2};
use sub_edit::History;
use sub_model::{ClipId, Project, Sequence, TrackId};
use sub_time::{Rational, RationalTime};
use sub_ui::selection::{ClipRef, apply_move};
use sub_ui::shortcuts::{Action, ShortcutMap};
use sub_ui::snapping::SnapSettings;
use sub_ui::split::apply_split;
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::TimelinePanel;

/// Where the fixture's clips sit on the first video track, in frames.
///
/// `shot 1` runs 0..96 and `shot 2` 96..168; the second video track carries
/// `lower third` at 24..72, over empty lane either side of it.
const SHOT_ONE_FRAMES: i64 = 96;
const SHOT_TWO_FRAMES: i64 = 72;
const LOWER_THIRD_START: i64 = 24;
const LOWER_THIRD_FRAMES: i64 = 48;

/// The frame `Ctrl+K` cuts at: inside `shot 1`, and clear of every snap
/// target so a cut there is unambiguous.
const CUT_AT: i64 = 40;

/// How far the drag test moves `lower third`, in frames.
const DRAG_FRAMES: i64 = 24;

/// The frame the single-frame snapshot is centred on.
const ZOOMED_FRAME: i64 = 96;

/// What a harness in this file carries: the panel, the project it paints, and
/// the undo stack every planned edit is applied through.
struct Scene {
    panel: TimelinePanel,
    project: Project,
    sequence: Sequence,
    history: History,
    /// The keyboard map, polled each frame exactly as the application polls it.
    shortcuts: ShortcutMap,
    /// The label of the last group that reached the history.
    applied: Option<String>,
}

impl Scene {
    /// The rate the fixture's first sequence is cut at.
    fn rate(&self) -> Rational {
        self.sequence.settings.frame_rate
    }

    /// `value` frames on the sequence's timebase.
    fn frames(&self, value: i64) -> RationalTime {
        RationalTime::new(value, self.rate())
    }

    /// The identity of track `index` in the sequence being painted.
    fn track(&self, index: usize) -> TrackId {
        self.sequence.tracks[index].id
    }

    /// Where every clip on track `index` starts and how long it runs, in
    /// frames, read back from the project rather than from the panel.
    fn spans(&self, index: usize) -> Vec<(i64, i64)> {
        let rate = self.rate();
        self.project.sequences[0].tracks[index]
            .placements(rate)
            .filter(|(item, _)| item.as_clip().is_some())
            .map(|(_, range)| (range.start().value(), range.duration().value()))
            .collect()
    }

    /// The clips on track `index`, in order.
    fn clips(&self, index: usize) -> Vec<ClipId> {
        self.project.sequences[0].tracks[index]
            .items
            .iter()
            .filter_map(|item| item.as_clip().map(|clip| clip.id))
            .collect()
    }

    /// The clips the panel has selected, in selection order.
    fn selected(&self) -> Vec<ClipId> {
        self.panel
            .selection()
            .items()
            .iter()
            .map(|item| item.clip)
            .collect()
    }

    /// Re-reads the sequence the panel paints after an edit, the way the
    /// application does when the engine bumps its revision.
    fn resync(&mut self) {
        self.sequence = self.project.sequences[0].clone();
        self.panel.invalidate();
    }

    /// Undoes the last group.
    ///
    /// # Panics
    ///
    /// Panics when there is nothing to undo, which is the assertion.
    fn undo(&mut self) {
        self.history
            .undo(&mut self.project)
            .expect("the undo applies")
            .expect("there was a step to undo");
        self.resync();
    }
}

/// A harness painting the fixture's first sequence at one pixel a frame.
///
/// One pixel a frame keeps the whole 168-frame cut on screen, so a lane
/// offset in points is a frame number and the tests can name positions
/// instead of computing them.
fn harness<'a>() -> egui_kittest::Harness<'a, Scene> {
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project).clone();
    let mut panel = TimelinePanel::new(sequence.settings.frame_rate);
    panel.view_mut().set_zoom(ZoomLevel::ONE);
    let state = Scene {
        panel,
        project,
        sequence,
        history: History::new(),
        shortcuts: ShortcutMap::default_map(),
        applied: None,
    };
    let mut harness = support::panel_harness_state(state, |ui, scene| {
        // The application polls the keyboard before it paints the dock, and
        // routes what fired to the panel that owns the state. Only the two
        // arms this suite drives are wired up here; the rest are the other
        // panels' business.
        for action in scene.shortcuts.poll(ui.ctx()) {
            match action {
                Action::SplitAtPlayhead => scene.panel.request_split_at_playhead(),
                Action::ToggleSnapping => {
                    scene.panel.toggle_snapping();
                }
                _ => {}
            }
        }

        let revision = scene.history.undo_len() as u64 + 1;
        scene.panel.sync(&scene.sequence, revision);
        let response = scene.panel.ui(ui, &scene.project, &scene.sequence);

        // Neither the drag nor the cut mutates anything itself: the panel
        // plans, and the plan is applied here through the Command API so the
        // whole gesture is one entry in the undo stack.
        if let Some(group) = response.clip_move {
            apply_move(&mut scene.history, &mut scene.project, &group)
                .expect("the planned drag applies");
            scene.applied = Some(group.label.clone());
            scene.resync();
        }
        if let Some(group) = response.clip_split {
            apply_split(&mut scene.history, &mut scene.project, &group)
                .expect("the planned cut applies");
            scene.applied = Some(group.label.clone());
            scene.resync();
        }
    });
    harness.run();
    harness
}

/// The point in track `track`'s lane `offset` points right of the lanes.
///
/// # Panics
///
/// Panics before the panel has painted once, which the harness above does.
fn lane_at(scene: &Scene, track: usize, offset: f32) -> Pos2 {
    let layout = scene.panel.layout().expect("the panel has painted");
    #[expect(
        clippy::cast_precision_loss,
        reason = "the fixture has three tracks, not billions"
    )]
    let pitch = scene.panel.metrics().lane_pitch() * track as f32;
    egui::pos2(
        layout.content.left() + offset,
        layout.content.top() + pitch + scene.panel.metrics().track_height / 2.0,
    )
}

/// The point on the ruler `offset` points right of the start of the lanes.
///
/// # Panics
///
/// Panics before the panel has painted once.
fn ruler_at(scene: &Scene, offset: f32) -> Pos2 {
    let layout = scene.panel.layout().expect("the panel has painted");
    egui::pos2(layout.content.left() + offset, layout.ruler.center().y)
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

/// Presses at `from`, drags through `to` and releases there.
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

/// A whole number of frames as a lane offset in points.
///
/// Only ever called with the small frame numbers this file's constants name.
#[expect(
    clippy::cast_precision_loss,
    reason = "the fixture is 168 frames long, not billions"
)]
fn at_frame(frame: i64) -> f32 {
    frame as f32
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

#[test]
fn an_empty_sequence_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let project = support::fixture_project();
    let settings = support::fixture_sequence(&project).settings;
    // What "new sequence" gives the user: the right timebase, and nothing on
    // it. The ruler, the empty lane area and the placeholder are the whole
    // picture.
    let sequence = Sequence::new("Empty", settings);
    let rate = sequence.settings.frame_rate;
    let mut panel = TimelinePanel::new(rate);
    let mut harness = support::panel_harness(|ui| {
        panel.sync(&sequence, 1);
        panel.ui(ui, &project, &sequence);
    });
    harness.run();
    support::snapshot(&mut harness, "timeline_empty_sequence");
}

#[test]
fn the_whole_cut_fitted_to_the_window_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project).clone();
    let rate = sequence.settings.frame_rate;
    let mut panel = TimelinePanel::new(rate);
    // The lane width is only known once the panel has laid itself out, so the
    // fit is applied on the second frame, which is what the "fit to window"
    // command does in the editor too.
    // The panel keeps the flag: the closure owns it for as long as the
    // harness lives, so what the frame did is read back through a cell.
    let fitted = Cell::new(false);
    let mut harness = support::panel_harness(|ui| {
        panel.sync(&sequence, 1);
        if !fitted.get() && panel.view().width_px() > 0 {
            let duration = panel.content_duration();
            panel.view_mut().fit(duration);
            fitted.set(true);
        }
        panel.ui(ui, &project, &sequence);
    });
    harness.run();
    harness.run();
    assert!(
        fitted.get(),
        "the view was fitted before the frame was rendered"
    );
    support::snapshot(&mut harness, "timeline_fit_to_window");
}

#[test]
fn a_single_frame_around_the_playhead_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project).clone();
    let rate = sequence.settings.frame_rate;
    let playhead = RationalTime::new(ZOOMED_FRAME, rate);
    let mut panel = TimelinePanel::new(rate);
    panel.set_playhead(playhead);
    let zoomed = Cell::new(false);
    let resolvable = Cell::new(false);
    let mut harness = support::panel_harness(|ui| {
        panel.sync(&sequence, 1);
        if !zoomed.get() && panel.view().width_px() > 0 {
            // All the way in, then scrolled back half a viewport so the cut
            // the playhead sits on is in the middle of the picture rather
            // than against its left edge.
            panel.view_mut().zoom_to_frame(playhead);
            let half = i64::from(panel.view().width_px() / 2);
            let left = panel.view().scroll_px() - half;
            panel.view_mut().set_scroll_px(left);
            resolvable.set(panel.view().zoom().is_frame_resolvable());
            zoomed.set(true);
        }
        panel.ui(ui, &project, &sequence);
    });
    harness.run();
    harness.run();
    assert!(
        zoomed.get(),
        "the view was zoomed before the frame was rendered"
    );
    assert!(
        resolvable.get(),
        "a single frame is wide enough to see at this zoom"
    );
    support::snapshot(&mut harness, "timeline_frame_zoom");
}

// ---------------------------------------------------------------------------
// Interactions
// ---------------------------------------------------------------------------

#[test]
fn clicking_a_clip_selects_the_clip_the_project_holds() {
    let mut harness = harness();
    assert!(
        harness.state().panel.selection().is_empty(),
        "a fresh panel has nothing selected"
    );

    let inside_shot_one = lane_at(harness.state(), 0, at_frame(20));
    click(&mut harness, inside_shot_one);

    let expected = harness.state().clips(0)[0];
    assert_eq!(
        harness.state().selected(),
        vec![expected],
        "the first clip on the top track is the one under the pointer"
    );

    // And a click on empty lane past the end of the cut clears it again.
    let past_the_end = lane_at(harness.state(), 1, at_frame(LOWER_THIRD_START + 200));
    click(&mut harness, past_the_end);
    assert!(
        harness.state().panel.selection().is_empty(),
        "clicking empty lane deselects"
    );
}

#[test]
fn dragging_a_clip_moves_it_in_one_undoable_step() {
    let mut harness = harness();
    // Snapping off, so the clip lands exactly where the pointer put it and
    // the assertion is a number rather than a nearby edge.
    *harness.state_mut().panel.snap_settings_mut() = SnapSettings::off();
    let from = lane_at(harness.state(), 1, at_frame(LOWER_THIRD_START + 10));
    let to = lane_at(
        harness.state(),
        1,
        at_frame(LOWER_THIRD_START + 10 + DRAG_FRAMES),
    );
    click(&mut harness, from);
    let clip = harness.state().clips(1)[0];
    assert_eq!(harness.state().selected(), vec![clip]);

    drag(&mut harness, from, to);

    assert_eq!(
        harness.state().applied.as_deref(),
        Some("Move clip"),
        "the released drag committed one labelled group"
    );
    assert_eq!(
        harness.state().spans(1),
        vec![(LOWER_THIRD_START + DRAG_FRAMES, LOWER_THIRD_FRAMES)],
        "the clip moved by the pointer's distance and kept its length"
    );
    assert_eq!(
        harness.state().history.undo_len(),
        1,
        "a whole drag is one entry in the undo stack"
    );

    harness.state_mut().undo();
    assert_eq!(
        harness.state().spans(1),
        vec![(LOWER_THIRD_START, LOWER_THIRD_FRAMES)],
        "and one undo puts it back"
    );
}

#[test]
fn ctrl_k_cuts_the_selected_clip_at_the_playhead() {
    let mut harness = harness();
    let track = harness.state().track(0);
    let clip = harness.state().clips(0)[0];
    // Only the top track's clip is selected, so the cut goes through that one
    // alone and the other tracks are the control.
    harness
        .state_mut()
        .panel
        .selection_mut()
        .select_only(ClipRef::new(track, clip));
    let playhead = harness.state().frames(CUT_AT);
    harness.state_mut().panel.set_playhead(playhead);

    harness.key_press_modifiers(Modifiers::COMMAND, Key::K);
    harness.run();
    harness.run();

    assert_eq!(
        harness.state().applied.as_deref(),
        Some("Split clip"),
        "the shortcut reached the panel and the panel planned a cut"
    );
    assert_eq!(
        harness.state().spans(0),
        vec![
            (0, CUT_AT),
            (CUT_AT, SHOT_ONE_FRAMES - CUT_AT),
            (SHOT_ONE_FRAMES, SHOT_TWO_FRAMES),
        ],
        "the head keeps its place and the new tail is butt joined to it"
    );
    assert_eq!(
        harness.state().clips(0)[0],
        clip,
        "the head keeps the clip's identity"
    );
    assert_eq!(
        harness.state().spans(1),
        vec![(LOWER_THIRD_START, LOWER_THIRD_FRAMES)],
        "the unselected track under the playhead was left alone"
    );
    assert_eq!(harness.state().history.undo_len(), 1);

    harness.state_mut().undo();
    assert_eq!(
        harness.state().spans(0),
        vec![(0, SHOT_ONE_FRAMES), (SHOT_ONE_FRAMES, SHOT_TWO_FRAMES)],
        "and one undo rejoins the two halves"
    );
}

#[test]
fn s_toggles_snapping_and_the_next_click_shows_it() {
    let mut harness = harness();
    assert!(
        harness.state().panel.snap_settings().enabled,
        "snapping is on until the user turns it off"
    );

    // Three pixels past the cut between the two clips on the top track: near
    // enough to be pulled onto it while snapping is on.
    let near_cut = ruler_at(harness.state(), at_frame(SHOT_ONE_FRAMES + 3));
    click(&mut harness, near_cut);
    assert_eq!(
        harness.state().panel.playhead(),
        harness.state().frames(SHOT_ONE_FRAMES),
        "the click was pulled onto the cut"
    );

    harness.key_press(Key::S);
    harness.run();
    assert!(
        !harness.state().panel.snap_settings().enabled,
        "S turns snapping off"
    );

    click(&mut harness, near_cut);
    assert_eq!(
        harness.state().panel.playhead(),
        harness.state().frames(SHOT_ONE_FRAMES + 3),
        "with snapping off the playhead lands where the pointer is"
    );

    harness.key_press(Key::S);
    harness.run();
    assert!(
        harness.state().panel.snap_settings().enabled,
        "and S turns it back on"
    );
    click(&mut harness, near_cut);
    assert_eq!(
        harness.state().panel.playhead(),
        harness.state().frames(SHOT_ONE_FRAMES),
        "the cut pulls again"
    );
}
