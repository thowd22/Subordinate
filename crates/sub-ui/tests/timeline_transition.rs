//! Crossfades on the timeline: the region they draw, and dragging their
//! duration.
//!
//! These drive the panel through the shared `egui_kittest` harness in
//! `tests/support`, the same frame every other panel test paints into, and
//! assert on the model rather than on pixels: what the transition's offsets
//! are after a drag, and what undoing one puts back. The one snapshot at the
//! bottom is the exception — it is there so the drawn region has a committed
//! picture.
//!
//! Every committed drag is applied through a real [`History`](sub_edit::History)
//! with `transition::apply_transition`, because "one undoable command" is only
//! true if undoing once puts the old blend back.

mod support;

use eframe::egui::{self, Pos2};
use sub_edit::History;
use sub_model::media::{StreamInfo, VideoStream};
use sub_model::sequence::SequenceSettings;
use sub_model::{
    Clip, ClipId, ColorTags, MediaItem, MediaPath, Project, Sequence, Track, TrackItem, TrackKind,
    Transition,
};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::TimelinePanel;
use sub_ui::transition::{TransitionEdge, TransitionRef, TransitionRefusal, apply_transition};

/// The sequence timebase every test here uses.
const RATE: Rational = Rational::FPS_24;

/// How long each clip on the top track is, in frames.
const CLIP_FRAMES: i64 = 48;

/// How far into the source the second clip starts, so its head has handle.
const TAIL_SOURCE_START: i64 = 240;

/// How long the probed source is, in frames.
const SOURCE_FRAMES: i64 = 480;

/// The blend the scene starts with, in frames, split evenly about the cut.
const BLEND_FRAMES: i64 = 12;

fn frames(value: i64) -> RationalTime {
    RationalTime::new(value, RATE)
}

fn range(start: i64, duration: i64) -> TimeRange {
    TimeRange::new(frames(start), frames(duration)).expect("a valid range")
}

/// The clips the scene is built from, so the tests can name them. The
/// crossfade is addressed by the clip it fades into, which is the tail.
struct Ids {
    tail: ClipId,
}

/// A project with two video tracks:
///
/// - V1: `head` at 0..48 and `tail` at 48..96, butt-joined, with a 12-frame
///   crossfade at the cut; both are cut from the middle of a probed 480-frame
///   file, so both sides of the cut have handle,
/// - V2: locked, holding the same cut and crossfade.
fn scene() -> (Project, Sequence, Ids) {
    let mut project = Project::new("crossfade");
    let mut item = MediaItem::new(MediaPath::new("media/take.mp4").expect("a valid path"));
    item.info = Some(StreamInfo {
        duration: Some(frames(SOURCE_FRAMES)),
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
    let head = Clip::new("head", media, range(48, CLIP_FRAMES));
    let tail = Clip::new("tail", media, range(TAIL_SOURCE_START, CLIP_FRAMES));
    let ids = Ids { tail: tail.id };
    let blend = Transition::crossfade(frames(BLEND_FRAMES / 2), frames(BLEND_FRAMES / 2));

    let mut top = Track::new("V1", TrackKind::Video);
    top.items.push(TrackItem::Clip(head));
    top.items.push(TrackItem::Transition(blend));
    top.items.push(TrackItem::Clip(tail));
    sequence.tracks.push(top);

    let mut locked = Track::new("V2", TrackKind::Video);
    locked.locked = true;
    locked.items.push(TrackItem::Clip(Clip::new(
        "a",
        media,
        range(48, CLIP_FRAMES),
    )));
    locked.items.push(TrackItem::Transition(blend));
    locked.items.push(TrackItem::Clip(Clip::new(
        "b",
        media,
        range(TAIL_SOURCE_START, CLIP_FRAMES),
    )));
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
    /// The label of the last crossfade edit the panel committed.
    applied: Option<String>,
    /// The last refusal the panel reported while a drag was in progress.
    refused: Option<TransitionRefusal>,
    /// The undo stack the committed edits land in.
    history: History,
}

impl Scene {
    /// The crossfade on the top track, as the project now holds it.
    fn blend(&self) -> Transition {
        self.project.sequences[0].tracks[0]
            .items
            .iter()
            .find_map(|item| item.as_transition().copied())
            .expect("the crossfade is still on the track")
    }

    /// The crossfade's offsets in frames, before and after the cut.
    fn offsets(&self) -> (i64, i64) {
        let blend = self.blend();
        (blend.in_offset().value(), blend.out_offset().value())
    }

    /// The crossfade the tests drag.
    fn target(&self) -> TransitionRef {
        TransitionRef::new(self.sequence.tracks[0].id, self.ids.tail)
    }

    /// Undoes the last committed edit.
    ///
    /// # Panics
    ///
    /// Panics when there is nothing to undo, which is the assertion.
    fn undo(&mut self) {
        self.history
            .undo(&mut self.project)
            .expect("one undo")
            .expect("there was a step to undo");
        self.sequence = self.project.sequences[0].clone();
        self.panel.invalidate();
    }
}

/// A harness painting the timeline panel over [`scene`] at one pixel a frame.
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
        history: History::new(),
    };
    let mut harness = support::panel_harness_state(state, |ui, scene| {
        scene
            .panel
            .sync(&scene.sequence, scene.history.undo_len() as u64 + 1);
        let response = scene.panel.ui(ui, &scene.project, &scene.sequence);
        if let Some(refusal) = response.transition_refused {
            scene.refused = Some(refusal);
        }
        if let Some(drag) = response.transition {
            apply_transition(&mut scene.history, &mut scene.project, &drag)
                .expect("the planned crossfade applies");
            scene.applied = Some(drag.label());
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
        reason = "the scene has two tracks, not billions"
    )]
    let pitch = scene.panel.metrics().lane_pitch() * track as f32;
    egui::pos2(
        layout.content.left() + offset,
        layout.content.top() + pitch + scene.panel.metrics().track_height / 2.0,
    )
}

/// A point inside the crossfade region on `track`, in the half `edge` names.
///
/// The zoom is one pixel a frame, so a frame number is a pixel offset; the cut
/// sits at frame 48 and the blend covers 42..54.
fn blend_at(scene: &Scene, track: usize, edge: TransitionEdge) -> Pos2 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "the scene's cut sits a few tens of frames along"
    )]
    let offset = match edge {
        TransitionEdge::Head => (CLIP_FRAMES - BLEND_FRAMES / 4) as f32,
        TransitionEdge::Tail => (CLIP_FRAMES + BLEND_FRAMES / 4) as f32,
    };
    lane_at(scene, track, offset)
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

/// Moves `by` points along the lane from `pos`.
fn along(pos: Pos2, by: f32) -> Pos2 {
    egui::pos2(pos.x + by, pos.y)
}

#[test]
fn hovering_a_crossfade_offers_its_edges() {
    let mut harness = harness();
    let tail = blend_at(harness.state(), 0, TransitionEdge::Tail);
    harness.hover_at(tail);
    harness.run();
    assert_eq!(
        harness.state().panel.hovered_transition(),
        Some((harness.state().target(), TransitionEdge::Tail)),
        "the pointer is in the half of the blend after the cut"
    );
    assert_eq!(
        harness.state().panel.hovered_trim(),
        None,
        "a crossfade owns the cut it covers, edges of the clips included"
    );

    let head = blend_at(harness.state(), 0, TransitionEdge::Head);
    harness.hover_at(head);
    harness.run();
    assert_eq!(
        harness
            .state()
            .panel
            .hovered_transition()
            .map(|(_, edge)| edge),
        Some(TransitionEdge::Head),
    );

    // Away from the blend there is nothing to grab.
    harness.hover_at(lane_at(harness.state(), 0, 12.0));
    harness.run();
    assert_eq!(harness.state().panel.hovered_transition(), None);

    // Nor on a locked track.
    harness.hover_at(blend_at(harness.state(), 1, TransitionEdge::Tail));
    harness.run();
    assert_eq!(harness.state().panel.hovered_transition(), None);
}

#[test]
fn dragging_the_tail_edge_lengthens_the_blend_about_the_cut() {
    let mut harness = harness();
    let tail = blend_at(harness.state(), 0, TransitionEdge::Tail);
    drag(&mut harness, tail, along(tail, 6.0));

    assert_eq!(
        harness.state().applied.as_deref(),
        Some("Change crossfade duration")
    );
    assert_eq!(
        harness.state().offsets(),
        (12, 12),
        "six points on one edge is twelve frames of blend, split about the cut"
    );
    assert_eq!(
        harness.state().project.sequences[0].tracks[0].items.len(),
        3,
        "the track still holds two clips and one transition"
    );
}

#[test]
fn dragging_the_head_edge_outwards_lengthens_it_too() {
    let mut harness = harness();
    let head = blend_at(harness.state(), 0, TransitionEdge::Head);
    drag(&mut harness, head, along(head, -4.0));
    assert_eq!(harness.state().offsets(), (10, 10));
}

#[test]
fn dragging_an_edge_inwards_shortens_the_blend() {
    let mut harness = harness();
    let tail = blend_at(harness.state(), 0, TransitionEdge::Tail);
    drag(&mut harness, tail, along(tail, -2.0));
    assert_eq!(harness.state().offsets(), (4, 4));
}

#[test]
fn a_drag_that_closes_the_blend_is_refused_and_changes_nothing() {
    let mut harness = harness();
    let tail = blend_at(harness.state(), 0, TransitionEdge::Tail);
    drag(&mut harness, tail, along(tail, -30.0));

    assert_eq!(harness.state().refused, Some(TransitionRefusal::TooShort));
    assert_eq!(harness.state().applied, None);
    assert_eq!(
        harness.state().offsets(),
        (BLEND_FRAMES / 2, BLEND_FRAMES / 2),
        "the blend is as it was"
    );
}

#[test]
fn a_dragged_crossfade_undoes_in_one_step() {
    let mut harness = harness();
    let tail = blend_at(harness.state(), 0, TransitionEdge::Tail);
    drag(&mut harness, tail, along(tail, 6.0));
    assert_eq!(harness.state().offsets(), (12, 12));

    harness.state_mut().undo();
    harness.run();
    assert_eq!(
        harness.state().offsets(),
        (BLEND_FRAMES / 2, BLEND_FRAMES / 2),
        "one undo puts the old blend back"
    );
}

#[test]
fn a_crossfade_on_a_locked_track_cannot_be_dragged() {
    let mut harness = harness();
    let locked = blend_at(harness.state(), 1, TransitionEdge::Tail);
    drag(&mut harness, locked, along(locked, 6.0));

    assert_eq!(harness.state().applied, None);
    let offsets = harness.state().project.sequences[0].tracks[1]
        .items
        .iter()
        .find_map(|item| item.as_transition().copied())
        .map(|blend| (blend.in_offset().value(), blend.out_offset().value()));
    assert_eq!(offsets, Some((BLEND_FRAMES / 2, BLEND_FRAMES / 2)));
}

#[test]
fn the_timeline_panel_with_a_crossfade_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let mut harness = harness();
    let tail = blend_at(harness.state(), 0, TransitionEdge::Tail);
    harness.hover_at(tail);
    harness.run();
    support::snapshot(&mut harness, "timeline_crossfade");
}
