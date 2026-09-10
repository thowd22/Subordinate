//! Cutting clips on the timeline: `Ctrl+K` at the playhead, and the razor.
//!
//! These drive the panel through the shared `egui_kittest` harness in
//! `tests/support`, the same frame every other panel test paints into, and
//! assert on the model rather than on pixels: where the clips are afterwards,
//! and what one undo puts back. The one snapshot at the bottom is the
//! exception — it is there so the razor tool has a committed picture.
//!
//! Every cut the panel plans is applied here the way the application will
//! apply it, through `split::apply_split` and a real
//! [`History`](sub_edit::History), because "one undoable cut" is only true if
//! undoing once rejoins every clip the cut went through.

mod support;

use eframe::egui::{self, Pos2};
use sub_edit::History;
use sub_model::media::{StreamInfo, VideoStream};
use sub_model::sequence::SequenceSettings;
use sub_model::{
    Clip, ClipId, ColorTags, Gap, Marker, MarkerId, MediaItem, MediaPath, Project, Sequence, Track,
    TrackItem, TrackKind,
};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::selection::ClipRef;
use sub_ui::split::{SplitRefusal, apply_split};
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::{TimelinePanel, Tool};

/// The sequence timebase every test here uses.
const RATE: Rational = Rational::FPS_24;

/// Where the clips on the first track sit, in frames.
const HEAD_START: i64 = 0;
const HEAD_FRAMES: i64 = 48;
const TAIL_START: i64 = 96;
const TAIL_FRAMES: i64 = 48;

/// The clip on the second track spans both of the first track's clips.
const UNDER_FRAMES: i64 = 144;

/// Where a cut through the stack lands.
const CUT_AT: i64 = 24;

/// Where the marker the razor snaps onto sits.
const MARKER_AT: i64 = 110;

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
    under: ClipId,
}

/// A project with three video tracks:
///
/// - V1: `head` at 0..48 and `tail` at 96..144,
/// - V2: `under` at 0..144, so a cut at [`CUT_AT`] goes through two tracks,
/// - V3: locked, and empty.
fn scene() -> (Project, Sequence, Ids) {
    let mut project = Project::new("split");
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

    let head = Clip::new("head", media, range(0, HEAD_FRAMES));
    let tail = Clip::new("tail", media, range(0, TAIL_FRAMES));
    let under = Clip::new("under", media, range(0, UNDER_FRAMES));
    let ids = Ids {
        head: head.id,
        tail: tail.id,
        under: under.id,
    };

    let mut top = Track::new("V1", TrackKind::Video);
    top.items.push(TrackItem::Clip(head));
    top.items
        .push(TrackItem::Gap(Gap::new(frames(TAIL_START - HEAD_FRAMES))));
    top.items.push(TrackItem::Clip(tail));
    sequence.tracks.push(top);

    let mut middle = Track::new("V2", TrackKind::Video);
    middle.items.push(TrackItem::Clip(under));
    sequence.tracks.push(middle);

    let mut locked = Track::new("V3", TrackKind::Video);
    locked.locked = true;
    sequence.tracks.push(locked);

    // A marker inside `tail`, for the razor to snap onto. Its identifier is
    // fixed because the colour it is painted in is derived from it, and the
    // snapshot below must be the same picture on every run.
    let mut marker = Marker::new("cue", TimeRange::empty_at(frames(MARKER_AT)));
    marker.id = MarkerId::parse("0192c4d0-0000-7000-8000-000000000001").expect("a valid id");
    sequence.markers.push(marker);

    project.sequences.push(sequence.clone());
    (project, sequence, ids)
}

/// What a harness in this file carries.
struct Scene {
    panel: TimelinePanel,
    project: Project,
    sequence: Sequence,
    ids: Ids,
    /// The label of the last cut the panel committed.
    applied: Option<String>,
    /// How many clips the last committed cut went through.
    cut_count: usize,
    /// The tail identities the last committed cut minted, in order.
    tails: Vec<ClipId>,
    /// The last refusal the panel reported.
    refused: Option<SplitRefusal>,
    /// The undo stack the committed cuts land in.
    history: History,
}

impl Scene {
    /// Where every clip on `track` starts and how long it is, in frames.
    fn spans(&self, track: usize) -> Vec<(i64, i64)> {
        self.project.sequences[0].tracks[track]
            .placements(RATE)
            .filter(|(item, _)| item.as_clip().is_some())
            .map(|(_, range)| (range.start().value(), range.duration().value()))
            .collect()
    }

    /// The clip ids on `track`, in order.
    fn clips(&self, track: usize) -> Vec<ClipId> {
        self.project.sequences[0].tracks[track]
            .items
            .iter()
            .filter_map(TrackItem::as_clip)
            .map(|clip| clip.id)
            .collect()
    }

    /// Undoes the last committed cut.
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
        cut_count: 0,
        tails: Vec::new(),
        refused: None,
        history: History::new(),
    };
    let mut harness = support::panel_harness_state(state, |ui, scene| {
        scene
            .panel
            .sync(&scene.sequence, scene.history.undo_len() as u64 + 1);
        let response = scene.panel.ui(ui, &scene.project, &scene.sequence);
        if let Some(refusal) = response.split_refused {
            scene.refused = Some(refusal);
        }
        if let Some(group) = response.clip_split {
            apply_split(&mut scene.history, &mut scene.project, &group)
                .expect("the planned cut applies");
            scene.applied = Some(group.label.clone());
            scene.cut_count = group.len();
            scene.tails = group.cuts.iter().map(|cut| cut.tail).collect();
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

/// Puts the playhead at `frame` and asks for the cut `Ctrl+K` asks for.
fn split_at_playhead(harness: &mut egui_kittest::Harness<'_, Scene>, frame: i64) {
    harness.state_mut().panel.set_playhead(frames(frame));
    harness.state_mut().panel.request_split_at_playhead();
    harness.run();
}

#[test]
fn a_cut_at_the_playhead_leaves_two_clips_where_there_was_one() {
    let mut harness = harness();
    // Only the first track's clip is selected, so the cut touches that alone.
    let track = harness.state().sequence.tracks[0].id;
    let head = harness.state().ids.head;
    harness
        .state_mut()
        .panel
        .selection_mut()
        .select_only(ClipRef::new(track, head));
    split_at_playhead(&mut harness, CUT_AT);

    let scene = harness.state();
    assert_eq!(scene.applied.as_deref(), Some("Split clip"));
    assert_eq!(scene.cut_count, 1);
    assert_eq!(
        scene.spans(0),
        vec![
            (HEAD_START, CUT_AT),
            (CUT_AT, HEAD_FRAMES - CUT_AT),
            (TAIL_START, TAIL_FRAMES),
        ],
        "the head keeps its place and the tail is butt joined to it"
    );
    let clips = scene.clips(0);
    assert_eq!(clips[0], head, "the head keeps the clip's identity");
    assert_eq!(
        clips[1], scene.tails[0],
        "and the tail gets the identity the plan minted"
    );
    assert_ne!(clips[0], clips[1]);
    assert_eq!(
        scene.spans(1),
        vec![(0, UNDER_FRAMES)],
        "the clip under the playhead on another track was not selected"
    );
}

#[test]
fn a_cut_at_the_playhead_is_one_undoable_step() {
    let mut harness = harness();
    split_at_playhead(&mut harness, CUT_AT);
    assert_eq!(
        harness.state().cut_count,
        2,
        "with nothing selected the cut goes through every editable track"
    );
    assert_eq!(harness.state().applied.as_deref(), Some("Split 2 clips"));
    assert_eq!(harness.state().history.undo_len(), 1, "one entry, not two");

    harness.state_mut().undo();
    let scene = harness.state();
    assert_eq!(
        scene.spans(0),
        vec![(HEAD_START, HEAD_FRAMES), (TAIL_START, TAIL_FRAMES)],
        "one undo rejoins the first track"
    );
    assert_eq!(
        scene.spans(1),
        vec![(0, UNDER_FRAMES)],
        "and the second one with it"
    );
    assert_eq!(scene.clips(0)[0], scene.ids.head);
    assert_eq!(scene.clips(1)[0], scene.ids.under);
}

#[test]
fn a_cut_that_lands_on_a_clips_own_edge_is_not_an_edit() {
    let mut harness = harness();
    // Frame 96 is where `tail` begins: there is an edit there already, and
    // cutting a clip on its own first frame would leave an empty head.
    let track = harness.state().sequence.tracks[0].id;
    let tail = harness.state().ids.tail;
    harness
        .state_mut()
        .panel
        .selection_mut()
        .select_only(ClipRef::new(track, tail));
    split_at_playhead(&mut harness, TAIL_START);
    let scene = harness.state();
    assert!(
        scene.applied.is_none(),
        "cutting on an existing edit must not push an undo step"
    );
    assert_eq!(scene.history.undo_len(), 0);
    assert!(scene.refused.is_none(), "and it is not a refusal either");
}

#[test]
fn the_razor_cuts_the_clip_it_is_clicked_on() {
    let mut harness = harness();
    harness.state_mut().panel.set_tool(Tool::Razor);
    // Frame 120: inside `tail`, and far enough from every snap target that
    // the click lands where it was aimed.
    let pos = lane_at(harness.state(), 0, 120.0);
    click(&mut harness, pos);

    let scene = harness.state();
    assert_eq!(scene.applied.as_deref(), Some("Split clip"));
    assert_eq!(
        scene.spans(0),
        vec![
            (HEAD_START, HEAD_FRAMES),
            (TAIL_START, 120 - TAIL_START),
            (120, TAIL_START + TAIL_FRAMES - 120),
        ],
    );
    assert_eq!(
        scene.spans(1),
        vec![(0, UNDER_FRAMES)],
        "the razor cuts the clip it was clicked on and nothing else"
    );
    assert_eq!(scene.clips(0)[1], scene.ids.tail, "the head keeps its id");
}

#[test]
fn the_razor_snaps_onto_a_marker() {
    let mut harness = harness();
    harness.state_mut().panel.set_tool(Tool::Razor);
    // Three points to the right of the marker at 110, which is inside the
    // default eight-point threshold: the cut belongs on the marker.
    let aimed = 113.0;
    let pos = lane_at(harness.state(), 0, aimed);
    harness.hover_at(pos);
    harness.run();
    assert_eq!(
        harness.state().panel.razor_time(),
        Some(frames(MARKER_AT)),
        "the cut line shows the snapped instant before the press"
    );
    click(&mut harness, pos);

    let scene = harness.state();
    assert_eq!(
        scene.spans(0),
        vec![
            (HEAD_START, HEAD_FRAMES),
            (TAIL_START, MARKER_AT - TAIL_START),
            (MARKER_AT, TAIL_START + TAIL_FRAMES - MARKER_AT),
        ],
        "the cut landed on the marker, not on the frame under the pointer"
    );
}

#[test]
fn the_razor_will_not_cut_a_locked_track() {
    let mut harness = harness();
    // Put a clip on the locked track through the model, so there is one there
    // to aim at at all.
    let clip = Clip::new("bolted", harness.state().project.media[0].id, range(0, 48));
    harness.state_mut().sequence.tracks[2]
        .items
        .push(TrackItem::Clip(clip));
    harness.state_mut().panel.invalidate();
    harness.state_mut().panel.set_tool(Tool::Razor);
    harness.run();

    let pos = lane_at(harness.state(), 2, 20.0);
    click(&mut harness, pos);
    assert!(
        harness.state().applied.is_none(),
        "a locked track refuses clip edits, so the razor does not cut it"
    );
    assert_eq!(harness.state().history.undo_len(), 0);
}

#[test]
fn putting_the_razor_away_drops_the_cut_line() {
    let mut harness = harness();
    harness.state_mut().panel.set_tool(Tool::Razor);
    let pos = lane_at(harness.state(), 0, 120.0);
    harness.hover_at(pos);
    harness.run();
    assert!(harness.state().panel.razor_time().is_some());

    harness.state_mut().panel.set_tool(Tool::Select);
    harness.run();
    assert_eq!(harness.state().panel.tool(), Tool::Select);
    assert!(
        harness.state().panel.razor_time().is_none(),
        "the arrow makes no promise about a cut"
    );
}

#[test]
fn the_timeline_panel_with_the_razor_out_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let mut harness = harness();
    harness.state_mut().panel.set_tool(Tool::Razor);
    let pos = lane_at(harness.state(), 0, 120.0);
    harness.hover_at(pos);
    harness.run();
    harness.run();
    assert_eq!(
        harness.state().panel.razor_time(),
        Some(frames(120)),
        "the picture is of a cut line at frame 120"
    );
    support::snapshot(&mut harness, "timeline_razor_tool");
}
