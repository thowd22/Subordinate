//! Selecting clips on the timeline, and dragging them to a new place.
//!
//! These drive the panel through the shared `egui_kittest` harness in
//! `tests/support`, the same frame every other panel test paints into, and
//! assert on the model rather than on pixels: what is selected afterwards, and
//! what commands the released drag produced. The one snapshot at the bottom is
//! the exception — it is there so the selected state has a committed picture.
//!
//! The drag assertions go one step further than the panel: the group it hands
//! back is applied through a real [`History`](sub_edit::History), because
//! "one grouped `MoveClip` command" is only true if undoing once puts every
//! dragged clip back.

mod support;

use eframe::egui::{self, Modifiers, Pos2};
use sub_edit::History;
use sub_model::media::{StreamInfo, VideoStream};
use sub_model::sequence::SequenceSettings;
use sub_model::{
    Clip, ClipId, ColorTags, Gap, MediaItem, MediaPath, Project, Sequence, Track, TrackItem,
    TrackKind,
};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::selection::{ClipRef, MoveRefusal, apply_move};
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::TimelinePanel;

/// The sequence timebase every test here uses.
const RATE: Rational = Rational::FPS_24;

/// Where the clips on the top track sit, in frames.
const HEAD_START: i64 = 0;
const TAIL_START: i64 = 96;
const CLIP_FRAMES: i64 = 48;

/// Where the clip on the second track sits.
const SECOND_TRACK_CLIP_START: i64 = 200;

fn frames(value: i64) -> RationalTime {
    RationalTime::new(value, RATE)
}

fn range(start: i64, duration: i64) -> TimeRange {
    TimeRange::new(frames(start), frames(duration)).expect("a valid range")
}

/// The clips the scene is built from, so the tests can name them.
struct Ids {
    head: ClipId,
    tail: ClipId,
    other: ClipId,
}

/// A project with three video tracks:
///
/// - V1: `head` at 0..48 and `tail` at 96..144,
/// - V2: `other` at 200..248,
/// - V3: locked, and empty.
fn scene() -> (Project, Sequence, Ids) {
    let mut project = Project::new("selection");
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

    let head = Clip::new("head", media, range(0, CLIP_FRAMES));
    let tail = Clip::new("tail", media, range(0, CLIP_FRAMES));
    let ids = Ids {
        head: head.id,
        tail: tail.id,
        other: ClipId::new(),
    };
    let mut top = Track::new("V1", TrackKind::Video);
    top.items.push(TrackItem::Clip(head));
    top.items
        .push(TrackItem::Gap(Gap::new(frames(TAIL_START - CLIP_FRAMES))));
    top.items.push(TrackItem::Clip(tail));
    sequence.tracks.push(top);

    let mut middle = Track::new("V2", TrackKind::Video);
    let mut other = Clip::new("other", media, range(0, CLIP_FRAMES));
    other.id = ids.other;
    middle
        .items
        .push(TrackItem::Gap(Gap::new(frames(SECOND_TRACK_CLIP_START))));
    middle.items.push(TrackItem::Clip(other));
    sequence.tracks.push(middle);

    let mut locked = Track::new("V3", TrackKind::Video);
    locked.locked = true;
    sequence.tracks.push(locked);

    project.sequences.push(sequence.clone());
    (project, sequence, ids)
}

/// What a harness in this file carries.
struct Scene {
    panel: TimelinePanel,
    project: Project,
    sequence: Sequence,
    ids: Ids,
    /// The last drag the panel committed, applied through the history below.
    applied: Option<String>,
    /// The last refusal the panel reported while a drag was in progress.
    refused: Option<MoveRefusal>,
    /// How many frames reported a selection change.
    selection_changes: u32,
    /// The undo stack the committed drags land in.
    history: History,
}

impl Scene {
    /// The selected clips, as bare clip ids in selection order.
    fn selected(&self) -> Vec<ClipId> {
        self.panel
            .selection()
            .items()
            .iter()
            .map(|item| item.clip)
            .collect()
    }

    /// Undoes the last committed drag.
    ///
    /// # Panics
    ///
    /// Panics when there is nothing to undo, which is the assertion.
    fn undo(&mut self) {
        self.history
            .undo(&mut self.project)
            .expect("one undo")
            .expect("there was a step to undo");
    }

    /// Where every clip on `track` starts, in frames.
    fn starts(&self, track: usize) -> Vec<i64> {
        self.project.sequences[0].tracks[track]
            .placements(RATE)
            .filter(|(item, _)| item.as_clip().is_some())
            .map(|(_, range)| range.start().value())
            .collect()
    }
}

/// A harness painting the timeline panel over [`scene`] at one pixel a frame.
///
/// The panel's plan is applied here the way the application will apply it:
/// through `selection::apply_move`, inside one history group.
fn harness<'a>() -> egui_kittest::Harness<'a, Scene> {
    let (project, sequence, ids) = scene();
    let mut panel = TimelinePanel::new(RATE);
    panel.view_mut().set_zoom(ZoomLevel::ONE);
    let state = Scene {
        panel,
        project,
        sequence,
        ids,
        applied: None,
        refused: None,
        selection_changes: 0,
        history: History::new(),
    };
    let mut harness = support::panel_harness_state(state, |ui, scene| {
        scene
            .panel
            .sync(&scene.sequence, scene.history.undo_len() as u64 + 1);
        let response = scene.panel.ui(ui, &scene.project, &scene.sequence);
        if response.selection_changed {
            scene.selection_changes += 1;
        }
        scene.refused = response.refused;
        if let Some(group) = response.clip_move {
            apply_move(&mut scene.history, &mut scene.project, &group)
                .expect("the planned drag applies");
            scene.applied = Some(group.label.clone());
            // The edit changed the sequence the panel is painting, exactly as
            // the engine's revision counter will when the app owns one.
            scene.sequence = scene.project.sequences[0].clone();
            scene.panel.invalidate();
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
        reason = "the scene has three tracks, not billions"
    )]
    let pitch = scene.panel.metrics().lane_pitch() * track as f32;
    egui::pos2(
        layout.content.left() + offset,
        layout.content.top() + pitch + scene.panel.metrics().track_height / 2.0,
    )
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

/// The same, with shift held for the press.
fn shift_click(harness: &mut egui_kittest::Harness<'_, Scene>, pos: Pos2) {
    harness.event(egui::Event::ModifiersChanged(Modifiers::SHIFT));
    click(harness, pos);
    harness.event(egui::Event::ModifiersChanged(Modifiers::NONE));
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

#[test]
fn clicking_a_clip_selects_it_and_clicking_empty_lane_clears_it() {
    let mut harness = harness();
    assert!(
        harness.state().panel.selection().is_empty(),
        "a fresh panel has nothing selected"
    );

    let head = lane_at(harness.state(), 0, 20.0);
    click(&mut harness, head);
    assert_eq!(harness.state().selected(), vec![harness.state().ids.head]);
    assert!(harness.state().selection_changes > 0);

    // A second clip replaces the first: a plain click is not additive.
    #[expect(
        clippy::cast_precision_loss,
        reason = "the clip starts a small whole number of frames along"
    )]
    let tail = lane_at(harness.state(), 0, (TAIL_START + 10) as f32);
    click(&mut harness, tail);
    assert_eq!(harness.state().selected(), vec![harness.state().ids.tail]);

    // And the gap between them selects nothing.
    let gap = lane_at(harness.state(), 0, 70.0);
    click(&mut harness, gap);
    assert!(
        harness.state().panel.selection().is_empty(),
        "clicking empty lane deselects"
    );
}

#[test]
fn shift_clicking_extends_and_then_removes_from_the_selection() {
    let mut harness = harness();
    let head = lane_at(harness.state(), 0, 20.0);
    #[expect(
        clippy::cast_precision_loss,
        reason = "the clip starts a small whole number of frames along"
    )]
    let tail = lane_at(harness.state(), 0, (TAIL_START + 10) as f32);

    click(&mut harness, head);
    shift_click(&mut harness, tail);
    assert_eq!(
        harness.state().selected(),
        vec![harness.state().ids.head, harness.state().ids.tail],
        "shift-click adds to the selection"
    );

    shift_click(&mut harness, head);
    assert_eq!(
        harness.state().selected(),
        vec![harness.state().ids.tail],
        "and shift-clicking a selected clip takes it out again"
    );
}

#[test]
fn a_marquee_selects_across_tracks() {
    let mut harness = harness();
    // The band has to start on empty lane: a press on a clip is a drag of
    // that clip, not a rubber band. Frame 60 is inside the gap on the top
    // track, so the band starts between the two clips there.
    let from = lane_at(harness.state(), 0, 60.0);
    #[expect(
        clippy::cast_precision_loss,
        reason = "the clip starts a small whole number of frames along"
    )]
    let to = lane_at(harness.state(), 1, (SECOND_TRACK_CLIP_START + 20) as f32);
    drag(&mut harness, from, to);

    let selected = harness.state().selected();
    let ids = &harness.state().ids;
    assert!(
        selected.contains(&ids.tail),
        "the band covers the second clip on the top track: got {selected:?}"
    );
    assert!(
        selected.contains(&ids.other),
        "and reached the clip on the track below: got {selected:?}"
    );
    assert!(
        !selected.contains(&ids.head),
        "but not the clip it started to the right of: got {selected:?}"
    );
    assert_eq!(selected.len(), 2);
    assert!(
        harness.state().panel.marquee_rect().is_none(),
        "the band is gone once the button comes up"
    );
}

#[test]
fn a_marquee_is_painted_while_the_button_is_down() {
    let mut harness = harness();
    let from = lane_at(harness.state(), 0, 60.0);
    let to = lane_at(harness.state(), 1, 300.0);
    harness.hover_at(from);
    harness.run();
    harness.drag_at(from);
    harness.run();
    harness.hover_at(to);
    harness.run();
    let band = harness
        .state()
        .panel
        .marquee_rect()
        .expect("the band is being painted");
    assert!(band.width() > 200.0 && band.height() > 0.0, "got {band:?}");
    harness.drop_at(to);
    harness.run();
}

#[test]
fn a_marquee_that_covers_one_clip_leaves_the_rest_alone() {
    let mut harness = harness();
    #[expect(
        clippy::cast_precision_loss,
        reason = "the clip starts a small whole number of frames along"
    )]
    let from = lane_at(harness.state(), 0, (TAIL_START - 4) as f32);
    #[expect(
        clippy::cast_precision_loss,
        reason = "the clip starts a small whole number of frames along"
    )]
    let to = lane_at(harness.state(), 0, (TAIL_START + 4) as f32);
    drag(&mut harness, from, to);
    assert_eq!(harness.state().selected(), vec![harness.state().ids.tail]);
}

#[test]
fn dragging_a_clip_commits_one_grouped_move_command() {
    let mut harness = harness();
    let head = lane_at(harness.state(), 0, 20.0);
    let dropped = lane_at(harness.state(), 0, 44.0);
    click(&mut harness, head);
    drag(&mut harness, head, dropped);

    assert_eq!(
        harness.state().applied.as_deref(),
        Some("Move clip"),
        "the drag committed one labelled group"
    );
    assert_eq!(
        harness.state().starts(0),
        vec![HEAD_START + 24, TAIL_START],
        "the clip moved 24 frames right and its neighbour stayed put"
    );
    assert_eq!(harness.state().history.undo_len(), 1);

    harness.state_mut().undo();
    assert_eq!(
        harness.state().starts(0),
        vec![HEAD_START, TAIL_START],
        "and one undo puts it back"
    );
}

#[test]
fn dragging_a_multi_clip_selection_is_still_one_undo_step() {
    let mut harness = harness();
    let head = lane_at(harness.state(), 0, 20.0);
    #[expect(
        clippy::cast_precision_loss,
        reason = "the clip starts a small whole number of frames along"
    )]
    let tail = lane_at(harness.state(), 0, (TAIL_START + 10) as f32);
    click(&mut harness, head);
    shift_click(&mut harness, tail);
    assert_eq!(harness.state().selected().len(), 2);

    // Pressing on one of the selected clips keeps the whole selection: both
    // clips travel together.
    let dropped = lane_at(harness.state(), 0, 32.0);
    drag(&mut harness, head, dropped);

    assert_eq!(harness.state().applied.as_deref(), Some("Move 2 clips"));
    assert_eq!(
        harness.state().starts(0),
        vec![HEAD_START + 12, TAIL_START + 12],
        "both clips moved by the same 12 frames"
    );
    assert_eq!(
        harness.state().history.undo_len(),
        1,
        "two MoveClip commands, one undo step"
    );

    harness.state_mut().undo();
    assert_eq!(
        harness.state().starts(0),
        vec![HEAD_START, TAIL_START],
        "and one undo puts both back"
    );
}

#[test]
fn dragging_onto_another_track_names_the_destination() {
    let mut harness = harness();
    let head = lane_at(harness.state(), 0, 20.0);
    let below = lane_at(harness.state(), 1, 20.0);
    click(&mut harness, head);
    drag(&mut harness, head, below);

    assert_eq!(harness.state().applied.as_deref(), Some("Move clip"));
    assert_eq!(
        harness.state().starts(0),
        vec![TAIL_START],
        "the clip left the top track"
    );
    assert_eq!(
        harness.state().starts(1),
        vec![HEAD_START, SECOND_TRACK_CLIP_START],
        "and landed on the one below it"
    );
}

#[test]
fn dragging_onto_a_locked_track_is_refused() {
    let mut harness = harness();
    let head = lane_at(harness.state(), 0, 20.0);
    let locked = lane_at(harness.state(), 2, 20.0);
    click(&mut harness, head);

    harness.hover_at(head);
    harness.run();
    harness.drag_at(head);
    harness.run();
    harness.hover_at(locked);
    harness.run();
    assert_eq!(
        harness.state().refused,
        Some(MoveRefusal::LockedTrack),
        "the refusal is reported while the button is still down"
    );
    assert_eq!(
        harness.state().panel.drag_refusal(),
        Some(MoveRefusal::LockedTrack),
        "and the panel is painting it"
    );
    assert!(harness.state().panel.drag_preview().is_none());

    harness.drop_at(locked);
    harness.run();
    assert!(
        harness.state().applied.is_none(),
        "letting go over a locked track commits nothing"
    );
    assert_eq!(harness.state().history.undo_len(), 0);
    assert_eq!(harness.state().starts(0), vec![HEAD_START, TAIL_START]);
}

#[test]
fn dragging_before_the_start_of_the_sequence_is_refused() {
    let mut harness = harness();
    let head = lane_at(harness.state(), 0, 20.0);
    let before = lane_at(harness.state(), 0, 4.0);
    click(&mut harness, head);

    harness.hover_at(head);
    harness.run();
    harness.drag_at(head);
    harness.run();
    harness.hover_at(before);
    harness.run();
    assert_eq!(
        harness.state().refused,
        Some(MoveRefusal::BeforeStart),
        "the first clip starts at zero, so any drag left leaves the sequence"
    );

    harness.drop_at(before);
    harness.run();
    assert!(harness.state().applied.is_none());
    assert_eq!(harness.state().starts(0), vec![HEAD_START, TAIL_START]);
}

#[test]
fn a_clip_on_a_locked_track_cannot_be_selected() {
    let mut harness = harness();
    // Move a clip onto the locked track through the model, so there is one
    // there to click on at all.
    let clip = Clip::new("bolted", harness.state().project.media[0].id, range(0, 48));
    harness.state_mut().sequence.tracks[2]
        .items
        .push(TrackItem::Clip(clip));
    harness.state_mut().panel.invalidate();
    harness.run();

    let locked = lane_at(harness.state(), 2, 20.0);
    click(&mut harness, locked);
    assert!(
        harness.state().panel.selection().is_empty(),
        "a locked track refuses clip edits, so its clips are not selectable"
    );
}

#[test]
fn a_drag_that_goes_nowhere_is_not_an_edit() {
    let mut harness = harness();
    let head = lane_at(harness.state(), 0, 20.0);
    click(&mut harness, head);
    drag(&mut harness, head, head);
    assert!(
        harness.state().applied.is_none(),
        "a press that wobbled nowhere must not push an undo step"
    );
    assert_eq!(harness.state().history.undo_len(), 0);
    assert_eq!(
        harness.state().selected(),
        vec![harness.state().ids.head],
        "and the clip stays selected"
    );
}

#[test]
fn the_timeline_panel_with_a_selected_clip_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project).clone();
    let rate = sequence.settings.frame_rate;
    let mut panel = TimelinePanel::new(rate);
    // The fixture's first two clips on the first track, chosen from the model
    // so the picture follows the fixture rather than a hard-coded guess.
    let track = &sequence.tracks[0];
    let selected: Vec<ClipRef> = track
        .items
        .iter()
        .filter_map(TrackItem::as_clip)
        .take(2)
        .map(|clip| ClipRef::new(track.id, clip.id))
        .collect();
    panel.selection_mut().set(selected);
    let mut harness = support::panel_harness(|ui| {
        panel.sync(&sequence, 1);
        panel.ui(ui, &project, &sequence);
    });
    harness.run();
    support::snapshot(&mut harness, "timeline_selection");
}
