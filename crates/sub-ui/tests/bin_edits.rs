//! Getting media from the bin onto the timeline: drag-and-drop, and the
//! insert and overwrite edits at the playhead.
//!
//! These drive the bin and the timeline together through the shared
//! `egui_kittest` harness in `tests/support` — both panels are painted into
//! the one frame, exactly as the dock paints them — and assert on the project
//! rather than on pixels: which clips are on the track afterwards, where they
//! start, and that one undo puts the track back.
//!
//! Every edit the panels plan is applied here through a real
//! [`History`](sub_edit::History), because "one undoable command" is only true
//! if undoing once restores the track.

mod support;

use eframe::egui::{self, Pos2};
use sub_edit::History;
use sub_model::media::{AudioStream, StreamInfo, VideoStream};
use sub_model::sequence::SequenceSettings;
use sub_model::{
    Clip, ColorTags, MediaId, MediaItem, MediaPath, Project, Sequence, Track, TrackKind,
};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::media_bin::{MediaBinPanel, drag_source_id};
use sub_ui::source_edit::{EditMode, SourceRefusal, apply_source_edit};
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::TimelinePanel;

/// The sequence timebase every test here uses.
const RATE: Rational = Rational::FPS_24;

/// How long each bin item lasts, in frames.
const ITEM_FRAMES: i64 = 48;

/// How long the clip already on the video track lasts.
const EXISTING_FRAMES: i64 = 24;

/// How tall the bin's half of the frame is, in points.
const BIN_HEIGHT: f32 = 180.0;

fn frames(value: i64) -> RationalTime {
    RationalTime::new(value, RATE)
}

fn range(start: i64, duration: i64) -> TimeRange {
    TimeRange::new(frames(start), frames(duration)).expect("a valid range")
}

/// A probed item lasting [`ITEM_FRAMES`], with the streams asked for.
fn item(name: &str, path: &str, video: bool, audio: bool) -> MediaItem {
    let mut item = MediaItem::new(MediaPath::new(path).expect("a valid path"));
    name.clone_into(&mut item.name);
    item.info = Some(StreamInfo {
        duration: Some(frames(ITEM_FRAMES)),
        video: if video {
            vec![VideoStream {
                width: 1920,
                height: 1080,
                frame_rate: RATE,
                sample_aspect: Rational::ONE,
                color: ColorTags::REC709,
            }]
        } else {
            Vec::new()
        },
        audio: if audio {
            vec![AudioStream {
                channels: 2,
                sample_rate: 48_000,
            }]
        } else {
            Vec::new()
        },
    });
    item
}

/// The items the bin holds, so the tests can name them.
struct Ids {
    /// A camera file: picture and sound.
    picture: MediaId,
    /// A sound file: no picture at all.
    sound: MediaId,
}

/// A project with one video track carrying a clip at 0..24, one empty audio
/// track, and two items filed in the root bin.
fn scene() -> (Project, Sequence, Ids) {
    let mut project = Project::new("bin edits");
    let picture = item("Take 1", "footage/take.mp4", true, true);
    let sound = item("Room tone", "audio/room.wav", false, true);
    let ids = Ids {
        picture: picture.id,
        sound: sound.id,
    };
    project.root_bin.media.push(picture.id);
    project.root_bin.media.push(sound.id);
    project.media.push(picture);
    project.media.push(sound);

    let mut sequence = Sequence::new("edit", SequenceSettings::default());
    let mut video = Track::new("V1", TrackKind::Video);
    video
        .items
        .push(Clip::new("existing", ids.picture, range(0, EXISTING_FRAMES)).into());
    sequence.tracks.push(video);
    sequence.tracks.push(Track::new("A1", TrackKind::Audio));
    project.sequences.push(sequence.clone());
    (project, sequence, ids)
}

/// What a harness in this file carries.
struct Scene {
    bin: MediaBinPanel,
    panel: TimelinePanel,
    project: Project,
    sequence: Sequence,
    ids: Ids,
    /// The history every planned edit is applied through.
    history: History,
    /// The label of the last edit committed.
    applied: Option<String>,
    /// The last refusal either panel reported.
    refused: Option<SourceRefusal>,
}

impl Scene {
    /// The clips on track `track` as (name, start, duration) in frames.
    fn clips(&self, track: usize) -> Vec<(String, i64, i64)> {
        self.project.sequences[0].tracks[track]
            .clip_placements(RATE)
            .map(|(clip, range)| {
                (
                    clip.name.clone(),
                    range.start().rescaled_to(RATE).value(),
                    range.duration().rescaled_to(RATE).value(),
                )
            })
            .collect()
    }

    /// Applies `plan` and remembers what it was.
    fn commit(&mut self, plan: sub_ui::source_edit::PlannedEdit) {
        self.applied = Some(plan.label());
        apply_source_edit(&mut self.history, &mut self.project, plan)
            .expect("the planned edit applies");
        // The edit changed the sequence the panel paints, exactly as the
        // engine's revision counter will when the app owns one.
        self.sequence = self.project.sequences[0].clone();
        self.panel.invalidate();
    }

    /// Undoes the last committed edit.
    fn undo(&mut self) {
        self.history
            .undo(&mut self.project)
            .expect("one undo")
            .expect("there was a step to undo");
        self.sequence = self.project.sequences[0].clone();
        self.panel.invalidate();
    }
}

/// Expected layouts read better as owned tuples.
fn expect(clips: &[(&str, i64, i64)]) -> Vec<(String, i64, i64)> {
    clips
        .iter()
        .map(|(name, start, duration)| ((*name).to_owned(), *start, *duration))
        .collect()
}

/// A harness painting the bin above the timeline at one pixel a frame.
fn harness<'a>() -> egui_kittest::Harness<'a, Scene> {
    let (project, sequence, ids) = scene();
    let mut panel = TimelinePanel::new(RATE);
    panel.view_mut().set_zoom(ZoomLevel::ONE);
    let state = Scene {
        bin: MediaBinPanel::new(),
        panel,
        project,
        sequence,
        ids,
        history: History::new(),
        applied: None,
        refused: None,
    };
    let mut harness = support::panel_harness_state(state, |ui, scene| {
        ui.allocate_ui(egui::vec2(ui.available_width(), BIN_HEIGHT), |ui| {
            scene.bin.ui(ui, &scene.project);
        });
        let revision = scene.history.undo_len() as u64 + 1;
        scene.panel.sync(&scene.sequence, revision);
        let response = scene.panel.ui(ui, &scene.project, &scene.sequence);
        // Sticky: a refusal is reported on the one frame the button comes up
        // on, and the harness paints several more before the test looks.
        scene.refused = response.drop_refused.or(scene.refused);
        if let Some(plan) = response.source_edit {
            scene.commit(plan);
        }
    });
    harness.run();
    harness
}

/// The point in track `track`'s lane `offset` points right of the lanes.
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

/// Drags the bin row of `media` onto `to` and lets go.
fn drag_to(harness: &mut egui_kittest::Harness<'_, Scene>, media: MediaId, to: Pos2) {
    let from = row_center(harness, media);
    harness.hover_at(from);
    harness.run();
    harness.drag_at(from);
    harness.run();
    harness.hover_at(to);
    harness.run();
    harness.drop_at(to);
    harness.run();
}

/// Where the bin drew the row for `media`, found through the widget rects egui
/// remembers rather than by guessing at the layout.
fn row_center(harness: &egui_kittest::Harness<'_, Scene>, media: MediaId) -> Pos2 {
    harness
        .ctx
        .read_response(drag_source_id(media))
        .map(|response| response.interact_rect)
        .map_or_else(
            || panic!("the bin has not painted a row for that item"),
            |rect| rect.center(),
        )
}

#[test]
fn dropping_an_item_on_a_track_places_a_clip_at_the_drop_time() {
    let mut harness = harness();
    let media = harness.state().ids.picture;
    let target = lane_at(harness.state(), 0, 60.0);
    drag_to(&mut harness, media, target);

    assert_eq!(
        harness.state().clips(0),
        expect(&[
            ("existing", 0, EXISTING_FRAMES),
            ("Take 1", 60, ITEM_FRAMES)
        ]),
        "the clip lands at the frame it was dropped on"
    );
    assert_eq!(harness.state().applied.as_deref(), Some("Overwrite Take 1"));

    harness.state_mut().undo();
    harness.run();
    assert_eq!(
        harness.state().clips(0),
        expect(&[("existing", 0, EXISTING_FRAMES)]),
        "one undo takes the dropped clip away"
    );
}

#[test]
fn a_drop_that_lands_on_a_clip_overwrites_it() {
    let mut harness = harness();
    let media = harness.state().ids.picture;
    let target = lane_at(harness.state(), 0, 12.0);
    drag_to(&mut harness, media, target);

    assert_eq!(
        harness.state().clips(0),
        expect(&[("existing", 0, 12), ("Take 1", 12, ITEM_FRAMES)]),
        "the clip already there is trimmed back to the drop point"
    );
}

#[test]
fn an_audio_only_item_is_refused_by_a_video_track_and_taken_by_an_audio_one() {
    let mut harness = harness();
    let media = harness.state().ids.sound;
    let video_lane = lane_at(harness.state(), 0, 24.0);
    drag_to(&mut harness, media, video_lane);

    assert_eq!(
        harness.state().clips(0),
        expect(&[("existing", 0, EXISTING_FRAMES)]),
        "the video track is left exactly as it was"
    );
    assert_eq!(harness.state().refused, Some(SourceRefusal::NeedsPicture));
    assert!(
        SourceRefusal::NeedsPicture
            .message()
            .contains("audio track"),
        "the refusal hints at the track it belongs on"
    );

    harness.state_mut().refused = None;
    let audio_lane = lane_at(harness.state(), 1, 24.0);
    drag_to(&mut harness, media, audio_lane);
    assert_eq!(
        harness.state().clips(1),
        expect(&[("Room tone", 24, ITEM_FRAMES)]),
        "the audio track takes the same item"
    );
    assert_eq!(harness.state().refused, None, "and refuses nothing");
}

#[test]
fn comma_inserts_and_period_overwrites_at_the_playhead_on_the_target_track() {
    let mut harness = harness();
    let media = harness.state().ids.picture;
    // Point at the video track, which is what aims the keyboard edits, and
    // put the playhead inside the clip already there.
    harness.state_mut().panel.set_target_track(0);
    harness.state_mut().panel.set_playhead(frames(12));
    harness.run();

    let plan = {
        let scene = harness.state();
        scene
            .panel
            .plan_edit_at_playhead(&scene.project, &scene.sequence, media, EditMode::Insert)
            .expect("an insert at the playhead")
    };
    harness.state_mut().commit(plan);
    harness.run();
    assert_eq!(
        harness.state().clips(0),
        expect(&[
            ("existing", 0, 12),
            ("Take 1", 12, ITEM_FRAMES),
            ("existing", 12 + ITEM_FRAMES, 12),
        ]),
        "an insert splits the clip at the playhead and ripples its tail later"
    );

    harness.state_mut().undo();
    let plan = {
        let scene = harness.state();
        scene
            .panel
            .plan_edit_at_playhead(&scene.project, &scene.sequence, media, EditMode::Overwrite)
            .expect("an overwrite at the playhead")
    };
    harness.state_mut().commit(plan);
    harness.run();
    assert_eq!(
        harness.state().clips(0),
        expect(&[("existing", 0, 12), ("Take 1", 12, ITEM_FRAMES)]),
        "an overwrite writes over what is there instead of moving it"
    );
}

#[test]
fn a_locked_track_refuses_the_bin_and_the_playhead_edit_alike() {
    let mut harness = harness();
    let media = harness.state().ids.picture;
    harness.state_mut().project.sequences[0].tracks[0].locked = true;
    harness.state_mut().sequence = harness.state().project.sequences[0].clone();
    harness.state_mut().panel.invalidate();
    harness.run();

    let target = lane_at(harness.state(), 0, 60.0);
    drag_to(&mut harness, media, target);
    assert_eq!(
        harness.state().clips(0),
        expect(&[("existing", 0, EXISTING_FRAMES)]),
        "a locked track takes nothing from the bin"
    );

    let scene = harness.state();
    let refusal = scene
        .panel
        .plan_edit_at_playhead(&scene.project, &scene.sequence, media, EditMode::Insert)
        .expect_err("a locked track refuses the keyboard edit too");
    assert_eq!(refusal, SourceRefusal::LockedTrack);
}

#[test]
fn the_drop_target_drawn_under_a_bin_drag_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let mut harness = harness();
    let media = harness.state().ids.picture;
    let from = row_center(&harness, media);
    let target = lane_at(harness.state(), 0, 60.0);
    // Held over the lane, not released: this is the feedback an editor sees
    // before the button comes up.
    harness.hover_at(from);
    harness.run();
    harness.drag_at(from);
    harness.run();
    harness.hover_at(target);
    harness.run();
    assert!(
        harness.state().panel.drop_preview().is_some(),
        "the ghost of the drop is what the picture is of"
    );
    support::snapshot(&mut harness, "bin_drop_target");
}
