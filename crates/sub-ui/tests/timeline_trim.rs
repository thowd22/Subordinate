//! Dragging a clip's edges on the timeline: normal trim, and ripple trim.
//!
//! These drive the panel through the shared `egui_kittest` harness in
//! `tests/support`, the same frame every other panel test paints into, and
//! assert on the model rather than on pixels: what the trimmed clip spans
//! afterwards, and what the clips after it do. The one snapshot at the bottom
//! is the exception — it is there so the trim handles have a committed
//! picture.
//!
//! Every committed trim is applied through a real [`History`](sub_edit::History)
//! with `trim::apply_trim`, because "one grouped command" is only true if
//! undoing once puts the whole track back.

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
use sub_ui::selection::ClipRef;
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::{TRIM_HANDLE_PX, TimelinePanel};
use sub_ui::trim::{TrimEdge, TrimRefusal, apply_trim};

/// The sequence timebase every test here uses.
const RATE: Rational = Rational::FPS_24;

/// Where the clips on the top track sit, in frames.
const HEAD_START: i64 = 0;
const TAIL_START: i64 = 96;
const CLIP_FRAMES: i64 = 48;

/// How far into the source both clips start, so both their in points have
/// handle to give back.
const HEAD_SOURCE_START: i64 = 24;

/// How long the probed source is, in frames.
const SOURCE_FRAMES: i64 = 480;

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
}

/// A project with two video tracks:
///
/// - V1: `head` at 0..48 and `tail` at 96..144, both cut from source 24..72,
/// - V2: locked, and empty.
fn scene() -> (Project, Sequence, Ids) {
    let mut project = Project::new("trim");
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
    let head = Clip::new("head", media, range(HEAD_SOURCE_START, CLIP_FRAMES));
    let tail = Clip::new("tail", media, range(HEAD_SOURCE_START, CLIP_FRAMES));
    let ids = Ids {
        head: head.id,
        tail: tail.id,
    };
    let mut top = Track::new("V1", TrackKind::Video);
    top.items.push(TrackItem::Clip(head));
    top.items
        .push(TrackItem::Gap(Gap::new(frames(TAIL_START - CLIP_FRAMES))));
    top.items.push(TrackItem::Clip(tail));
    sequence.tracks.push(top);

    let mut locked = Track::new("V2", TrackKind::Video);
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
    /// The label of the last trim the panel committed.
    applied: Option<String>,
    /// The last refusal the panel reported while a trim was in progress.
    refused: Option<TrimRefusal>,
    /// The undo stack the committed trims land in.
    history: History,
}

impl Scene {
    /// Every clip's span on the top track, in frames.
    fn spans(&self) -> Vec<(i64, i64)> {
        self.project.sequences[0].tracks[0]
            .placements(RATE)
            .filter(|(item, _)| item.as_clip().is_some())
            .map(|(_, range)| (range.start().value(), range.duration().value()))
            .collect()
    }

    /// The source range the clip `id` is cut from, in frames.
    fn source(&self, id: ClipId) -> (i64, i64) {
        let clip = self.project.sequences[0].tracks[0]
            .items
            .iter()
            .filter_map(TrackItem::as_clip)
            .find(|clip| clip.id == id)
            .expect("the clip is still on the track");
        (
            clip.source_range.start().value(),
            clip.source_range.duration().value(),
        )
    }

    /// Undoes the last committed trim.
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
        if let Some(refusal) = response.trim_refused {
            scene.refused = Some(refusal);
        }
        if let Some(group) = response.clip_trim {
            apply_trim(&mut scene.history, &mut scene.project, &group)
                .expect("the planned trim applies");
            scene.applied = Some(group.label.clone());
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

/// The point on a clip's edge that a trim drag starts from.
///
/// The zoom is one pixel a frame, so a frame number is a pixel offset; the
/// grab lands just inside the edge, where the handle is.
fn edge_at(scene: &Scene, track: usize, frame: i64, edge: TrimEdge) -> Pos2 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "the scene's clips sit a few tens of frames along"
    )]
    let offset = frame as f32;
    let inset = TRIM_HANDLE_PX / 2.0;
    let offset = match edge {
        TrimEdge::In => offset + inset,
        TrimEdge::Out => offset - inset,
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

/// The same, with alt held for the whole gesture: a ripple trim.
fn alt_drag(harness: &mut egui_kittest::Harness<'_, Scene>, from: Pos2, to: Pos2) {
    harness.event(egui::Event::ModifiersChanged(Modifiers::ALT));
    drag(harness, from, to);
    harness.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    harness.run();
}

/// Moves `by` points along the lane from `pos`.
fn along(pos: Pos2, by: f32) -> Pos2 {
    egui::pos2(pos.x + by, pos.y)
}

#[test]
fn hovering_a_clip_edge_offers_the_trim_handle() {
    let mut harness = harness();
    let out_edge = edge_at(harness.state(), 0, CLIP_FRAMES, TrimEdge::Out);
    harness.hover_at(out_edge);
    harness.run();
    assert_eq!(
        harness.state().panel.hovered_trim(),
        Some((
            ClipRef::new(
                harness.state().sequence.tracks[0].id,
                harness.state().ids.head
            ),
            TrimEdge::Out
        )),
        "the pointer is on the head clip's out point"
    );

    let in_edge = edge_at(harness.state(), 0, HEAD_START, TrimEdge::In);
    harness.hover_at(in_edge);
    harness.run();
    assert_eq!(
        harness.state().panel.hovered_trim().map(|(_, edge)| edge),
        Some(TrimEdge::In),
        "and here on its in point"
    );

    // The middle of a clip is a move, not a trim.
    let middle = lane_at(harness.state(), 0, 24.0);
    harness.hover_at(middle);
    harness.run();
    assert_eq!(harness.state().panel.hovered_trim(), None);

    // So is empty lane.
    let gap = lane_at(harness.state(), 0, 70.0);
    harness.hover_at(gap);
    harness.run();
    assert_eq!(harness.state().panel.hovered_trim(), None);
}

#[test]
fn dragging_the_out_handle_trims_the_out_point() {
    let mut harness = harness();
    let out_edge = edge_at(harness.state(), 0, CLIP_FRAMES, TrimEdge::Out);
    drag(&mut harness, out_edge, along(out_edge, 12.0));

    assert_eq!(harness.state().applied.as_deref(), Some("Trim clip out"));
    assert_eq!(
        harness.state().spans(),
        vec![(HEAD_START, CLIP_FRAMES + 12), (TAIL_START, CLIP_FRAMES)],
        "the clip grew and its neighbour stayed where it was"
    );
    assert_eq!(
        harness.state().source(harness.state().ids.head),
        (HEAD_SOURCE_START, CLIP_FRAMES + 12),
        "twelve more frames of the source are used"
    );

    harness.state_mut().undo();
    assert_eq!(
        harness.state().spans(),
        vec![(HEAD_START, CLIP_FRAMES), (TAIL_START, CLIP_FRAMES)],
        "one undo puts the trim back"
    );
}

#[test]
fn dragging_the_in_handle_moves_the_head_and_keeps_the_out_point() {
    let mut harness = harness();
    let in_edge = edge_at(harness.state(), 0, HEAD_START, TrimEdge::In);
    drag(&mut harness, in_edge, along(in_edge, 12.0));

    assert_eq!(harness.state().applied.as_deref(), Some("Trim clip in"));
    assert_eq!(
        harness.state().spans(),
        vec![(12, CLIP_FRAMES - 12), (TAIL_START, CLIP_FRAMES)],
        "the head moved right and the out point stayed"
    );
    assert_eq!(
        harness.state().source(harness.state().ids.head),
        (HEAD_SOURCE_START + 12, CLIP_FRAMES - 12)
    );
}

#[test]
fn a_trim_is_clamped_to_the_source_it_has_left() {
    let mut harness = harness();
    // `tail` starts 24 frames into its source, so its in point has exactly 24
    // frames of handle to the left however far the drag goes. (`head` sits at
    // the head of the sequence, where a normal trim has nowhere to go at all.)
    let in_edge = edge_at(harness.state(), 0, TAIL_START, TrimEdge::In);
    drag(&mut harness, in_edge, along(in_edge, -200.0));

    assert_eq!(
        harness.state().source(harness.state().ids.tail),
        (0, CLIP_FRAMES + HEAD_SOURCE_START),
        "the in point stopped at the first frame of the media"
    );
    assert_eq!(
        harness.state().spans()[1],
        (
            TAIL_START - HEAD_SOURCE_START,
            CLIP_FRAMES + HEAD_SOURCE_START
        ),
        "and the clip could only grow by the handle it had"
    );
}

#[test]
fn holding_alt_ripples_the_clips_after_the_trim() {
    let mut harness = harness();
    let out_edge = edge_at(harness.state(), 0, CLIP_FRAMES, TrimEdge::Out);
    alt_drag(&mut harness, out_edge, along(out_edge, 12.0));

    assert_eq!(
        harness.state().applied.as_deref(),
        Some("Ripple trim clip out")
    );
    assert_eq!(
        harness.state().spans(),
        vec![
            (HEAD_START, CLIP_FRAMES + 12),
            (TAIL_START + 12, CLIP_FRAMES)
        ],
        "the clip after the trim moved by exactly what the edge moved"
    );

    harness.state_mut().undo();
    assert_eq!(
        harness.state().spans(),
        vec![(HEAD_START, CLIP_FRAMES), (TAIL_START, CLIP_FRAMES)],
        "one undo puts the trim and the clip it rippled back together"
    );
    assert_eq!(
        harness.state().history.undo_len(),
        0,
        "which means it was one entry, not two"
    );
}

#[test]
fn a_ripple_in_trim_keeps_the_head_where_it_was() {
    let mut harness = harness();
    let in_edge = edge_at(harness.state(), 0, HEAD_START, TrimEdge::In);
    alt_drag(&mut harness, in_edge, along(in_edge, 12.0));

    assert_eq!(
        harness.state().applied.as_deref(),
        Some("Ripple trim clip in")
    );
    assert_eq!(
        harness.state().spans(),
        vec![
            (HEAD_START, CLIP_FRAMES - 12),
            (TAIL_START - 12, CLIP_FRAMES)
        ],
        "the head stayed put, the clip shortened and the track closed up"
    );
    assert_eq!(
        harness.state().source(harness.state().ids.head),
        (HEAD_SOURCE_START + 12, CLIP_FRAMES - 12),
        "the frames that left are the ones at the head of the source"
    );
    assert_eq!(
        harness.state().source(harness.state().ids.tail),
        (HEAD_SOURCE_START, CLIP_FRAMES),
        "and the rippled clip is moved, not trimmed"
    );
}

#[test]
fn a_trim_that_goes_nowhere_is_not_an_edit() {
    let mut harness = harness();
    let out_edge = edge_at(harness.state(), 0, CLIP_FRAMES, TrimEdge::Out);
    drag(&mut harness, out_edge, out_edge);
    assert!(
        harness.state().applied.is_none(),
        "a press that wobbled nowhere must not push an undo step"
    );
    assert_eq!(harness.state().history.undo_len(), 0);
}

#[test]
fn a_clip_on_a_locked_track_offers_no_handle() {
    let mut harness = harness();
    let clip = Clip::new("bolted", harness.state().project.media[0].id, range(0, 48));
    harness.state_mut().sequence.tracks[1]
        .items
        .push(TrackItem::Clip(clip));
    harness.state_mut().panel.invalidate();
    harness.run();

    let locked = edge_at(harness.state(), 1, CLIP_FRAMES, TrimEdge::Out);
    harness.hover_at(locked);
    harness.run();
    assert_eq!(
        harness.state().panel.hovered_trim(),
        None,
        "a locked track refuses clip edits, so its edges are not grabbable"
    );
    assert!(harness.state().refused.is_none());
}

#[test]
fn the_timeline_panel_with_a_hovered_trim_handle_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let mut harness = harness();
    let out_edge = edge_at(harness.state(), 0, CLIP_FRAMES, TrimEdge::Out);
    harness.hover_at(out_edge);
    harness.run();
    assert!(
        harness.state().panel.hovered_trim().is_some(),
        "the picture is of a hovered handle, so there had better be one"
    );
    support::snapshot(&mut harness, "timeline_trim_handles");
}
