//! Dragging an audio clip's fade handles on the timeline.
//!
//! These drive the panel through the shared `egui_kittest` harness in
//! `tests/support`, the same frame every other panel test paints into, and
//! assert on the model rather than on pixels: how long the clip's fades are
//! afterwards, and what one undo puts back. The one test that does look at
//! pixels is the one that has to — the handles are painted shapes, and
//! "an audio clip shows a handle at both ends" is a claim about what is drawn.
//!
//! Every committed fade is applied through a real [`History`](sub_edit::History)
//! with `fade::apply_fade`, because "one undoable command" is only true if
//! undoing once puts the clip back.

mod support;

use eframe::egui::{self, Pos2, Rect};
use sub_edit::History;
use sub_model::sequence::SequenceSettings;
use sub_model::{
    Clip, ClipId, MediaItem, MediaPath, Project, Sequence, Track, TrackItem, TrackKind,
};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::fade::{FadeEdge, FadeRefusal, apply_fade};
use sub_ui::selection::ClipRef;
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::{FADE_ALPHA, FADE_BAND_PX, FADE_COLOR, TimelinePanel};

/// The sequence timebase every test here uses.
const RATE: Rational = Rational::FPS_24;

/// How long the clip on the audio track is, in frames.
const CLIP_FRAMES: i64 = 48;

fn frames(value: i64) -> RationalTime {
    RationalTime::new(value, RATE)
}

fn range(start: i64, duration: i64) -> TimeRange {
    TimeRange::new(frames(start), frames(duration)).expect("a valid range")
}

/// A project with one audio track holding a 48 frame clip at the head of the
/// sequence, and a locked audio track under it.
fn scene() -> (Project, Sequence, ClipId) {
    let mut project = Project::new("fades");
    let item = MediaItem::new(MediaPath::new("media/take.wav").expect("a valid path"));
    let media = item.id;
    project.media.push(item);

    let clip = Clip::new("take", media, range(0, CLIP_FRAMES));
    let clip_id = clip.id;
    let mut track = Track::new("A1", TrackKind::Audio);
    track.items.push(TrackItem::Clip(clip));

    let mut sequence = Sequence::new("edit", SequenceSettings::default());
    sequence.tracks.push(track);
    let mut locked = Track::new("A2", TrackKind::Audio);
    locked.locked = true;
    sequence.tracks.push(locked);

    project.sequences.push(sequence.clone());
    (project, sequence, clip_id)
}

/// What a harness in this file carries.
struct Scene {
    panel: TimelinePanel,
    project: Project,
    sequence: Sequence,
    clip: ClipId,
    /// The label of the last fade the panel committed.
    applied: Option<String>,
    /// The last refusal the panel reported while a drag was in progress.
    refused: Option<FadeRefusal>,
    /// The undo stack the committed fades land in.
    history: History,
}

impl Scene {
    /// The clip's two fades, in frames.
    fn fades(&self) -> (i64, i64) {
        let clip = self.project.sequences[0].tracks[0]
            .clip(self.clip)
            .expect("the clip is still on the track");
        (
            clip.fade_in.rescaled_to(RATE).value(),
            clip.fade_out.rescaled_to(RATE).value(),
        )
    }

    /// The clip the drags in this file take hold of.
    fn target(&self) -> ClipRef {
        ClipRef::new(self.sequence.tracks[0].id, self.clip)
    }

    /// Undoes the last committed fade.
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
    let (project, sequence, clip) = scene();
    let mut panel = TimelinePanel::new(RATE);
    panel.view_mut().set_zoom(ZoomLevel::ONE);
    let state = Scene {
        panel,
        project,
        sequence,
        clip,
        applied: None,
        refused: None,
        history: History::new(),
    };
    let mut harness = support::panel_harness_state(state, |ui, scene| {
        scene
            .panel
            .sync(&scene.sequence, scene.history.undo_len() as u64 + 1);
        let response = scene.panel.ui(ui, &scene.project, &scene.sequence);
        if let Some(refusal) = response.fade_refused {
            scene.refused = Some(refusal);
        }
        if let Some(edit) = response.clip_fade {
            apply_fade(&mut scene.history, &mut scene.project, &edit)
                .expect("the planned fade applies");
            scene.applied = Some(edit.label().to_owned());
            scene.sequence = scene.project.sequences[0].clone();
            scene.panel.invalidate();
        }
    });
    harness.run();
    harness
}

/// The point in the top lane `offset` points right of the lanes, `down` points
/// below the top of the lane.
fn lane_at(scene: &Scene, offset: f32, down: f32) -> Pos2 {
    let layout = scene.panel.layout().expect("the panel has painted");
    egui::pos2(layout.content.left() + offset, layout.content.top() + down)
}

/// The point a fade handle sitting `frame` frames along the clip is grabbed at.
///
/// The zoom is one pixel a frame, so a frame number is a pixel offset, and the
/// grab lands in the band across the top of the clip where the handles live.
fn handle_at(scene: &Scene, frame: i64) -> Pos2 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "the scene's clip is a few tens of frames long"
    )]
    // A point exactly on the left edge of the lanes belongs to the header
    // column beside them, so the head handle is aimed at a pixel inside.
    let offset = (frame as f32).max(1.0);
    lane_at(scene, offset, FADE_BAND_PX / 2.0)
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

/// Every filled rectangle in `shape`, however deeply nested, with its colour.
fn walk(shape: &egui::Shape, into: &mut Vec<(Rect, egui::Color32)>) {
    match shape {
        egui::Shape::Rect(rect) => into.push((rect.rect, rect.fill)),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                walk(shape, into);
            }
        }
        _ => {}
    }
}

/// Moves `by` points along the lane from `pos`.
fn along(pos: Pos2, by: f32) -> Pos2 {
    egui::pos2(pos.x + by, pos.y)
}

#[test]
fn hovering_the_top_of_an_audio_clip_offers_its_fade_handles() {
    let mut harness = harness();
    let head = handle_at(harness.state(), 0);
    harness.hover_at(head);
    harness.run();
    assert_eq!(
        harness.state().panel.hovered_fade(),
        Some((harness.state().target(), FadeEdge::In)),
        "the pointer is on the clip's fade in handle"
    );

    let tail = handle_at(harness.state(), CLIP_FRAMES);
    harness.hover_at(tail);
    harness.run();
    assert_eq!(
        harness.state().panel.hovered_fade().map(|(_, edge)| edge),
        Some(FadeEdge::Out),
        "and here on its fade out handle"
    );

    // Below the band, the same column is the clip's edge and so a trim.
    let below = lane_at(harness.state(), 1.0, FADE_BAND_PX + 8.0);
    harness.hover_at(below);
    harness.run();
    assert_eq!(harness.state().panel.hovered_fade(), None);
    assert!(
        harness.state().panel.hovered_trim().is_some(),
        "the clip's edge is still a trim below the fade band"
    );

    // And the middle of the clip is neither.
    let middle = handle_at(harness.state(), CLIP_FRAMES / 2);
    harness.hover_at(middle);
    harness.run();
    assert_eq!(harness.state().panel.hovered_fade(), None);
}

#[test]
fn dragging_the_head_handle_right_fades_the_clip_in() {
    let mut harness = harness();
    let head = handle_at(harness.state(), 0);
    drag(&mut harness, head, along(head, 12.0));

    assert_eq!(harness.state().applied.as_deref(), Some("Change fade in"));
    assert_eq!(
        harness.state().fades(),
        (12, 0),
        "the head ramps up over twelve frames and the tail is untouched"
    );

    harness.state_mut().undo();
    harness.run();
    assert_eq!(harness.state().fades(), (0, 0), "one undo puts it back");
}

#[test]
fn dragging_the_tail_handle_left_fades_the_clip_out() {
    let mut harness = harness();
    let tail = handle_at(harness.state(), CLIP_FRAMES);
    drag(&mut harness, tail, along(tail, -16.0));

    assert_eq!(harness.state().applied.as_deref(), Some("Change fade out"));
    assert_eq!(harness.state().fades(), (0, 16));

    harness.state_mut().undo();
    harness.run();
    assert_eq!(harness.state().fades(), (0, 0));
}

#[test]
fn a_fade_dragged_past_the_far_end_stops_at_the_clip() {
    let mut harness = harness();
    let head = handle_at(harness.state(), 0);
    drag(&mut harness, head, along(head, 400.0));
    assert_eq!(
        harness.state().fades(),
        (CLIP_FRAMES, 0),
        "the fade fills the clip and goes no further"
    );
    assert_eq!(
        harness.state().refused,
        None,
        "a clamped drag is not refused"
    );
}

#[test]
fn an_audio_clip_paints_a_fade_handle_at_both_ends() {
    let (project, sequence, _) = scene();
    let mut panel = TimelinePanel::new(RATE);
    panel.view_mut().set_zoom(ZoomLevel::ONE);
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 600.0))),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        panel.sync(&sequence, 1);
        panel.ui(ui, &project, &sequence);
    });
    let shapes = std::mem::take(&mut output.shapes);
    output.textures_delta.clear();

    let layout = panel.layout().expect("the panel has painted");
    let mut fills = Vec::new();
    for clipped in &shapes {
        walk(&clipped.shape, &mut fills);
    }
    let grips: Vec<_> = fills
        .into_iter()
        .filter(|(_, color)| {
            // The grips are painted through the same tint the ramps are.
            *color
                == egui::Color32::from_rgba_unmultiplied(
                    FADE_COLOR.r(),
                    FADE_COLOR.g(),
                    FADE_COLOR.b(),
                    FADE_ALPHA,
                )
        })
        .map(|(rect, _)| rect)
        .collect();
    assert_eq!(grips.len(), 2, "one grip at each end of the clip");

    let left = layout.content.left();
    #[expect(
        clippy::cast_precision_loss,
        reason = "the clip is a few tens of frames long"
    )]
    let clip_right = left + CLIP_FRAMES as f32;
    assert!(
        grips.iter().any(|rect| rect.center().x < left + 8.0),
        "a grip sits at the clip's head: {grips:?}"
    );
    assert!(
        grips.iter().any(|rect| rect.center().x > clip_right - 8.0),
        "a grip sits at the clip's tail: {grips:?}"
    );
}
