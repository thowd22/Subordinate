//! Getting media from the bin onto the timeline: insert and overwrite.
//!
//! Three-point editing without the third point, which is what the MVP needs
//! (docs/PLAN.md §5.1): the source is a whole media item, the destination is a
//! track, and the instant is either where the item was dropped or where the
//! playhead is. What differs is what happens to what is already there —
//! [`EditMode::Overwrite`] writes over it ([`AddClip`]), [`EditMode::Insert`]
//! pushes it later ([`InsertClip`]) — and both are one undoable command.
//!
//! Nothing here paints or mutates. A gesture arrives already reduced to a
//! media id, a track index and an instant in [`RationalTime`]; the plan that
//! comes back is either the command the edit will be, with the exact span it
//! will occupy so the panel can draw it, or a modelled [`SourceRefusal`] that
//! says why it cannot happen. That is what lets a refusal reach the user as a
//! hint before the button comes up rather than as silence afterwards.
//!
//! Track kinds are part of the refusal, not a filter: an item with no picture
//! belongs on an audio track, and dropping it on a video one is refused with a
//! hint pointing at the track it does belong on.

use sub_core::{SubError, SubResult};
use sub_edit::clip::{AddClip, InsertClip};
use sub_edit::commands::{InsertSequence, InsertTrack};
use sub_edit::{BoxedCommand, History};
use sub_model::{
    Clip, MediaId, MediaItem, Project, Sequence, SequenceId, Track, TrackId, TrackKind,
};
use sub_time::{RationalTime, TimeRange};

use crate::codes;
use crate::timeline_panel::clip_edits_allowed;

/// What an edit from the bin does to what is already on the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    /// Push everything from the edit point later by the clip's length.
    Insert,
    /// Write over the span the clip lands on.
    Overwrite,
}

impl EditMode {
    /// A stable identifier, for logging and for tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Overwrite => "overwrite",
        }
    }

    /// The verb this mode is described by, in menus and history entries.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Insert => "Insert",
            Self::Overwrite => "Overwrite",
        }
    }
}

/// Why an item from the bin cannot be edited onto a track.
///
/// Modelled rather than a boolean because the panel shows it: a refused drop
/// is painted differently and carries [`SourceRefusal::message`] as a tooltip,
/// so the editor is told what to do instead of watching nothing happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRefusal {
    /// The drop landed off the ends of the sequence's tracks.
    NoSuchTrack,
    /// The track is locked, so it takes no clips.
    LockedTrack,
    /// The item is not in this project.
    UnknownMedia,
    /// The item has never been probed, so its length is unknown.
    Unprobed,
    /// A video track was handed an item with no picture.
    NeedsPicture,
    /// An audio track was handed an item with no sound.
    NeedsSound,
    /// The edit point is before the head of the sequence.
    BeforeStart,
    /// The arithmetic of the placement overflowed, which no real edit does.
    OutOfRange,
}

impl SourceRefusal {
    /// A stable identifier, for logging and for tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::NoSuchTrack => "no_such_track",
            Self::LockedTrack => "locked_track",
            Self::UnknownMedia => "unknown_media",
            Self::Unprobed => "unprobed",
            Self::NeedsPicture => "needs_picture",
            Self::NeedsSound => "needs_sound",
            Self::BeforeStart => "before_start",
            Self::OutOfRange => "out_of_range",
        }
    }

    /// One sentence saying what is wrong and, where there is one, what to do
    /// instead.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NoSuchTrack => "the drop is not on a track of this sequence",
            Self::LockedTrack => "a locked track cannot take a clip",
            Self::UnknownMedia => "that item is not in this project",
            Self::Unprobed => "that item has not been probed yet, so its length is unknown",
            Self::NeedsPicture => "this item has no picture: drop it on an audio track",
            Self::NeedsSound => "this item has no sound: drop it on a video track",
            Self::BeforeStart => "a clip cannot start before the head of the sequence",
            Self::OutOfRange => "the placement is too far to represent",
        }
    }

    /// This refusal as a [`SubError`] carrying `ui.source_edit_refused`.
    #[must_use]
    pub fn to_error(self) -> SubError {
        SubError::new(codes::SOURCE_EDIT_REFUSED, self.message()).with_detail("reason", self.id())
    }
}

/// One planned edit from the bin: the command it will be, and the span it will
/// occupy.
///
/// The span is what the panel paints as the drop target, and it comes from the
/// same clip the command carries, so what is previewed and what is committed
/// cannot drift apart.
#[derive(Debug, Clone)]
pub struct PlannedEdit {
    /// Whether the edit inserts or overwrites.
    pub mode: EditMode,
    /// The sequence the edit lands in.
    pub sequence: SequenceId,
    /// The track the clip lands on.
    pub track: TrackId,
    /// The index of that track in [`Sequence::tracks`].
    pub track_index: usize,
    /// The clip that will be placed, identity and all.
    pub clip: Clip,
    /// The span the clip occupies in sequence time.
    pub range: TimeRange,
    create_sequence: Option<InsertSequence>,
    create_track: Option<InsertTrack>,
    companion_audio: Option<Box<PlannedEdit>>,
}

impl PlannedEdit {
    /// Where the clip starts, in sequence time.
    #[must_use]
    pub fn start(&self) -> RationalTime {
        self.range.start()
    }

    /// The label the one history entry gets.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{} {}", self.mode.label(), self.clip.name)
    }

    /// Commands for one undoable gesture, including any empty-timeline setup.
    #[must_use]
    pub fn into_commands(mut self) -> Vec<BoxedCommand> {
        let mut commands: Vec<BoxedCommand> = Vec::new();
        if let Some(command) = self.create_sequence.take() {
            commands.push(Box::new(command));
        }
        if let Some(command) = self.create_track.take() {
            commands.push(Box::new(command));
        }
        let companion = self.companion_audio.take();
        commands.push(self.into_clip_command());
        if let Some(audio) = companion {
            commands.extend(audio.into_commands());
        }
        commands
    }

    /// The command this edit is, ready for the Command API.
    #[must_use]
    fn into_clip_command(self) -> BoxedCommand {
        match self.mode {
            EditMode::Insert => Box::new(InsertClip {
                sequence: self.sequence,
                track: self.track,
                start: self.range.start(),
                clip: self.clip,
                tail_id: None,
            }),
            EditMode::Overwrite => Box::new(AddClip {
                sequence: self.sequence,
                track: self.track,
                start: self.range.start(),
                clip: self.clip,
            }),
        }
    }
}

/// Works out what editing `media` onto track `track_index` at `start` would
/// do.
///
/// `start` is an instant in sequence time — a drop position or the playhead —
/// and is rescaled to the sequence's timebase, so an edit always lands on a
/// frame boundary. The clip spans the whole of the item's probed duration at
/// the item's own rate, which is the source time the clip is a slice of.
///
/// # Errors
///
/// Returns the [`SourceRefusal`] that says why the edit cannot be made: the
/// track is missing or locked, the item is unknown or unprobed, the item and
/// the track are of different kinds, or the edit point is before zero.
pub fn plan_source_edit(
    project: &Project,
    sequence: &Sequence,
    media: MediaId,
    track_index: usize,
    start: RationalTime,
    mode: EditMode,
) -> Result<PlannedEdit, SourceRefusal> {
    let track = sequence
        .tracks
        .get(track_index)
        .ok_or(SourceRefusal::NoSuchTrack)?;
    if !clip_edits_allowed(track) {
        return Err(SourceRefusal::LockedTrack);
    }
    let item = project
        .media_item(media)
        .ok_or(SourceRefusal::UnknownMedia)?;
    check_kind(item, track.kind)?;
    let duration = item
        .info
        .as_ref()
        .and_then(|info| info.duration)
        .filter(|duration| !duration.is_zero() && !duration.is_negative())
        .ok_or(SourceRefusal::Unprobed)?;

    let start = start.rescaled_to(sequence.settings.frame_rate);
    if start.is_negative() {
        return Err(SourceRefusal::BeforeStart);
    }
    let source_range = TimeRange::new(RationalTime::zero(duration.rate()), duration)
        .ok_or(SourceRefusal::OutOfRange)?;
    let clip = Clip::new(item.name.clone(), item.id, source_range);
    let range = clip
        .timeline_range(start)
        .ok_or(SourceRefusal::OutOfRange)?;
    Ok(PlannedEdit {
        mode,
        sequence: sequence.id,
        track: track.id,
        track_index,
        clip,
        range,
        create_sequence: None,
        create_track: None,
        companion_audio: None,
    })
}

/// Plans a bin edit, creating a first lane only when the timeline has none.
/// The fallback sequence is inserted only when it is absent from the project.
/// A video drop carrying sound also places a synchronized clip on the first
/// audio lane, creating A1 when no audio lane exists. A locked audio lane
/// refuses the whole gesture. Dropping explicitly onto audio takes sound only.
/// The pair shares one history entry; it is not a persistent linked-edit group.
///
/// # Errors
/// Returns the same media and placement refusals as [`plan_source_edit`].
pub fn plan_timeline_source_edit(
    project: &Project,
    sequence: &Sequence,
    media: MediaId,
    track_index: usize,
    start: RationalTime,
    mode: EditMode,
) -> Result<PlannedEdit, SourceRefusal> {
    if !sequence.tracks.is_empty() {
        let plan = plan_source_edit(project, sequence, media, track_index, start, mode)?;
        return pair_source_audio(project, sequence, plan);
    }
    let item = project
        .media_item(media)
        .ok_or(SourceRefusal::UnknownMedia)?;
    let info = item.info.as_ref().ok_or(SourceRefusal::Unprobed)?;
    let (name, kind) = if info.has_video() {
        ("V1", TrackKind::Video)
    } else {
        ("A1", TrackKind::Audio)
    };
    let track = Track::new(name, kind);
    let mut destination = sequence.clone();
    destination.tracks.push(track.clone());
    let mut plan = plan_source_edit(project, &destination, media, 0, start, mode)?;
    if project.sequence(sequence.id).is_none() {
        plan.create_sequence = Some(InsertSequence::new(
            project.sequences.len(),
            sequence.clone(),
        ));
    }
    plan.create_track = Some(InsertTrack::new(sequence.id, 0, track));
    pair_source_audio(project, &destination, plan)
}

/// Pair normal picture drops with the source's default audio stream, so the
/// preview and exporter (both of which mix audio tracks) receive its sound.
fn pair_source_audio(
    project: &Project,
    sequence: &Sequence,
    mut plan: PlannedEdit,
) -> Result<PlannedEdit, SourceRefusal> {
    let has_audio = project
        .media_item(plan.clip.media)
        .and_then(|item| item.info.as_ref())
        .is_some_and(sub_model::StreamInfo::has_audio);
    if sequence.tracks[plan.track_index].kind != TrackKind::Video || !has_audio {
        return Ok(plan);
    }
    let mut destination = sequence.clone();
    let mut create_track = None;
    let audio_index = if let Some(index) = destination
        .tracks
        .iter()
        .position(|track| track.kind == TrackKind::Audio)
    {
        index
    } else {
        let index = destination.tracks.len();
        let track = Track::new("A1", TrackKind::Audio);
        create_track = Some(InsertTrack::new(sequence.id, index, track.clone()));
        destination.tracks.push(track);
        index
    };
    let mut audio = plan_source_edit(
        project,
        &destination,
        plan.clip.media,
        audio_index,
        plan.start(),
        plan.mode,
    )?;
    audio.create_track = create_track;
    plan.companion_audio = Some(Box::new(audio));
    Ok(plan)
}

/// Whether `item` has anything a track of `kind` can play.
///
/// A video track wants picture and an audio track wants sound; an item
/// carrying both is welcome on either, which is what makes dragging a camera
/// file onto an audio track lay down its sound.
fn check_kind(item: &MediaItem, kind: TrackKind) -> Result<(), SourceRefusal> {
    let Some(info) = item.info.as_ref() else {
        // An unprobed item has no streams to judge and no length either; the
        // length is the refusal the caller reports.
        return Ok(());
    };
    match kind {
        TrackKind::Video if !info.has_video() => Err(SourceRefusal::NeedsPicture),
        TrackKind::Audio if !info.has_audio() => Err(SourceRefusal::NeedsSound),
        _ => Ok(()),
    }
}

/// Applies a planned edit as one undoable step.
///
/// # Errors
///
/// Whatever the command returns: the media is gone, the track is locked, or
/// the placement is not representable.
pub fn apply_source_edit(
    history: &mut History,
    project: &mut Project,
    plan: PlannedEdit,
) -> SubResult<()> {
    history.begin_group(plan.label())?;
    for command in plan.into_commands() {
        history.apply_boxed(project, command)?;
    }
    history.commit_group()?;
    Ok(())
}

/// Turns a refusal into the error the Command API would have raised.
///
/// # Errors
///
/// Always: it is a refusal.
pub fn refuse<T>(refusal: SourceRefusal) -> SubResult<T> {
    Err(refusal.to_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_model::media::{AudioStream, StreamInfo, VideoStream};
    use sub_model::sequence::{ColorTags, SequenceSettings};
    use sub_model::{MediaPath, Track};
    use sub_time::Rational;

    const RATE: Rational = Rational::FPS_24;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, RATE)
    }

    /// A probed item lasting `length` frames, with the streams asked for.
    fn item(name: &str, length: i64, video: bool, audio: bool) -> MediaItem {
        let mut item = MediaItem::new(MediaPath::new("footage/a.mp4").expect("a valid path"));
        item.name = name.to_owned();
        item.info = Some(StreamInfo {
            duration: Some(frames(length)),
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

    /// A project with one video track, one audio track and the items given.
    fn fixture(items: Vec<MediaItem>) -> (Project, Sequence) {
        let mut project = Project::new("Source edits");
        project.media = items;
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.tracks.push(Track::new("V1", TrackKind::Video));
        sequence.tracks.push(Track::new("A1", TrackKind::Audio));
        project.sequences.push(sequence.clone());
        (project, sequence)
    }

    #[test]
    fn empty_timeline_bootstrap_matches_media_and_undoes_exactly() {
        for video in [true, false] {
            for existing_sequence in [true, false] {
                let source = item("source", 48, video, !video);
                let media = source.id;
                let mut project = Project::new("Untitled");
                project.media.push(source);
                let sequence = Sequence::new("Sequence", SequenceSettings::default());
                if existing_sequence {
                    project.sequences.push(sequence.clone());
                }
                let before = project.clone();
                let plan = plan_timeline_source_edit(
                    &project,
                    &sequence,
                    media,
                    4,
                    frames(24),
                    EditMode::Overwrite,
                )
                .expect("an empty timeline accepts a probed item");
                assert_eq!(project, before, "planning must not mutate the project");
                let mut history = History::new();
                apply_source_edit(&mut history, &mut project, plan).expect("bootstrap edit");
                let after = project.clone();
                assert_eq!(project.sequences.len(), 1);
                assert_eq!(project.sequences[0].tracks.len(), 1);
                assert_eq!(
                    project.sequences[0].tracks[0].kind,
                    if video {
                        TrackKind::Video
                    } else {
                        TrackKind::Audio
                    }
                );
                history
                    .undo(&mut project)
                    .expect("one undo removes the complete gesture");
                assert_eq!(project, before);
                history
                    .redo(&mut project)
                    .expect("redo restores all identities");
                assert_eq!(project, after);
            }
        }
    }

    #[test]
    fn audiovisual_drop_pairs_both_streams_and_undoes_every_created_entity() {
        for mode in [EditMode::Insert, EditMode::Overwrite] {
            for existing_sequence in [false, true] {
                let source = item("camera", 48, true, true);
                let media = source.id;
                let mut project = Project::new("Untitled");
                project.media.push(source);
                let sequence = Sequence::new("Main", SequenceSettings::default());
                if existing_sequence {
                    project.sequences.push(sequence.clone());
                }
                let before = project.clone();
                let plan =
                    plan_timeline_source_edit(&project, &sequence, media, 0, frames(24), mode)
                        .expect("an A/V drop");
                assert_eq!(project, before, "planning is read-only");
                let mut history = History::new();
                apply_source_edit(&mut history, &mut project, plan).unwrap();
                let tracks = &project.sequences[0].tracks;
                assert_eq!(tracks.len(), 2);
                assert_eq!(tracks[0].kind, TrackKind::Video);
                assert_eq!(tracks[1].kind, TrackKind::Audio);
                let (video, video_span) = tracks[0].clip_placements(RATE).next().unwrap();
                let (audio, audio_span) = tracks[1].clip_placements(RATE).next().unwrap();
                assert_ne!(video.id, audio.id);
                assert_eq!(video.media, media);
                assert_eq!(audio.media, media);
                assert_eq!(video.source_range, audio.source_range);
                assert_eq!(video_span, audio_span);
                assert_eq!(video_span.start(), frames(24));
                assert_eq!(
                    audio.audio_stream, 0,
                    "one default source stream, not a duplicate mix"
                );
                let after = project.clone();
                history.undo(&mut project).unwrap();
                assert_eq!(project, before);
                history.redo(&mut project).unwrap();
                assert_eq!(project, after);
            }
        }
    }

    #[test]
    fn paired_insert_and_overwrite_use_the_existing_audio_lane_and_respect_mute() {
        for mode in [EditMode::Insert, EditMode::Overwrite] {
            let source = item("camera", 48, true, true);
            let media = source.id;
            let (mut project, _) = fixture(vec![source]);
            project.sequences[0].tracks[1].muted = true;
            let audio_track = project.sequences[0].tracks[1].id;
            let mut history = History::new();
            for start in [frames(0), frames(24)] {
                let sequence = project.sequences[0].clone();
                let plan =
                    plan_timeline_source_edit(&project, &sequence, media, 0, start, mode).unwrap();
                apply_source_edit(&mut history, &mut project, plan).unwrap();
            }
            let tracks = &project.sequences[0].tracks;
            assert_eq!(tracks.len(), 2, "reuse the audio destination");
            assert_eq!(tracks[1].id, audio_track);
            assert!(
                tracks[1].muted,
                "placing source audio does not unmute a lane"
            );
            let video: Vec<_> = tracks[0]
                .clip_placements(RATE)
                .map(|(clip, span)| (clip.media, clip.source_range, span))
                .collect();
            let audio: Vec<_> = tracks[1]
                .clip_placements(RATE)
                .map(|(clip, span)| (clip.media, clip.source_range, span))
                .collect();
            assert_eq!(video, audio, "both streams receive the same edit mode");
        }
    }

    #[test]
    fn silent_video_and_explicit_audio_drops_do_not_create_companions() {
        for (video, audio, target) in [(true, false, 0), (true, true, 1), (false, true, 1)] {
            let source = item("source", 48, video, audio);
            let media = source.id;
            let (mut project, sequence) = fixture(vec![source]);
            let plan = plan_timeline_source_edit(
                &project,
                &sequence,
                media,
                target,
                frames(0),
                EditMode::Overwrite,
            )
            .unwrap();
            apply_source_edit(&mut History::new(), &mut project, plan).unwrap();
            assert_eq!(project.sequences[0].tracks[target].clips().count(), 1);
            assert_eq!(project.sequences[0].tracks[1 - target].clips().count(), 0);
            assert_eq!(project.sequences[0].tracks.len(), 2);
        }
    }

    #[test]
    fn locked_companion_refuses_the_pair_and_late_failure_rolls_back_picture() {
        let source = item("camera", 48, true, true);
        let media = source.id;
        let (mut project, mut sequence) = fixture(vec![source]);
        sequence.tracks[1].locked = true;
        assert_eq!(
            plan_timeline_source_edit(
                &project,
                &sequence,
                media,
                0,
                frames(0),
                EditMode::Overwrite
            )
            .unwrap_err(),
            SourceRefusal::LockedTrack
        );
        sequence.tracks[1].locked = false;
        let plan = plan_timeline_source_edit(
            &project,
            &sequence,
            media,
            0,
            frames(0),
            EditMode::Overwrite,
        )
        .unwrap();
        // The destination disappears between planning and committing. Picture
        // must not remain applied after the companion command fails.
        project.sequences[0].tracks.pop();
        let before = project.clone();
        let mut history = History::new();
        assert!(apply_source_edit(&mut history, &mut project, plan).is_err());
        assert_eq!(project, before);
        history.begin_group("no dangling group").unwrap();
        history.abort_group(&mut project).unwrap();
    }

    #[test]
    fn a_planned_overwrite_spans_the_whole_item_from_the_edit_point() {
        let source = item("shot", 48, true, true);
        let media = source.id;
        let (project, sequence) = fixture(vec![source]);
        let plan = plan_source_edit(
            &project,
            &sequence,
            media,
            0,
            frames(24),
            EditMode::Overwrite,
        )
        .expect("a video item on a video track");

        assert_eq!(plan.range.start(), frames(24));
        assert_eq!(plan.range.duration(), frames(48));
        assert_eq!(plan.clip.source_range.start(), frames(0));
        assert_eq!(plan.label(), "Overwrite shot");
    }

    #[test]
    fn an_audio_only_item_is_refused_by_a_video_track_with_a_hint() {
        let source = item("room tone", 48, false, true);
        let media = source.id;
        let (project, sequence) = fixture(vec![source]);
        let refusal = plan_source_edit(&project, &sequence, media, 0, frames(0), EditMode::Insert)
            .expect_err("a video track has no use for sound alone");
        assert_eq!(refusal, SourceRefusal::NeedsPicture);
        assert!(
            refusal.message().contains("audio track"),
            "the hint names the track it belongs on: {}",
            refusal.message()
        );
        assert_eq!(refusal.to_error().code, codes::SOURCE_EDIT_REFUSED);

        // The same item is welcome on the audio track.
        plan_source_edit(&project, &sequence, media, 1, frames(0), EditMode::Insert)
            .expect("an audio item on an audio track");
    }

    #[test]
    fn a_silent_item_is_refused_by_an_audio_track() {
        let source = item("titles", 24, true, false);
        let media = source.id;
        let (project, sequence) = fixture(vec![source]);
        let refusal = plan_source_edit(&project, &sequence, media, 1, frames(0), EditMode::Insert)
            .expect_err("an audio track has no use for a silent still");
        assert_eq!(refusal, SourceRefusal::NeedsSound);
    }

    #[test]
    fn a_locked_track_an_unknown_item_and_an_unprobed_one_are_refused() {
        let source = item("shot", 48, true, true);
        let media = source.id;
        let mut unprobed = item("unknown", 48, true, true);
        unprobed.info = None;
        let unprobed_id = unprobed.id;
        let (mut project, mut sequence) = fixture(vec![source, unprobed]);

        assert_eq!(
            plan_source_edit(
                &project,
                &sequence,
                MediaId::new(),
                0,
                frames(0),
                EditMode::Insert
            )
            .expect_err("an item that is not in the project"),
            SourceRefusal::UnknownMedia
        );
        assert_eq!(
            plan_source_edit(
                &project,
                &sequence,
                unprobed_id,
                0,
                frames(0),
                EditMode::Insert
            )
            .expect_err("an item of unknown length"),
            SourceRefusal::Unprobed
        );
        assert_eq!(
            plan_source_edit(&project, &sequence, media, 7, frames(0), EditMode::Insert)
                .expect_err("a lane past the last track"),
            SourceRefusal::NoSuchTrack
        );

        sequence.tracks[0].locked = true;
        project.sequences[0].tracks[0].locked = true;
        assert_eq!(
            plan_source_edit(&project, &sequence, media, 0, frames(0), EditMode::Insert)
                .expect_err("a locked track"),
            SourceRefusal::LockedTrack
        );
    }

    #[test]
    fn an_insert_ripples_and_an_overwrite_covers_what_is_there() {
        let source = item("shot", 24, true, true);
        let media = source.id;
        let (mut project, sequence) = fixture(vec![source]);
        let mut history = History::new();

        // Two clips laid end to end, then an edit at the cut between them.
        for _ in 0..2 {
            let plan = plan_source_edit(
                &project,
                &sequence,
                media,
                0,
                project.sequences[0].tracks[0].duration(RATE),
                EditMode::Overwrite,
            )
            .expect("a plan");
            apply_source_edit(&mut history, &mut project, plan).expect("an overwrite");
        }
        assert_eq!(
            project.sequences[0].tracks[0].duration(RATE),
            frames(48),
            "two 24 frame clips, laid end to end"
        );

        let plan = plan_source_edit(&project, &sequence, media, 0, frames(24), EditMode::Insert)
            .expect("a plan");
        apply_source_edit(&mut history, &mut project, plan).expect("an insert");
        assert_eq!(
            project.sequences[0].tracks[0].duration(RATE),
            frames(72),
            "an insert makes room rather than covering the second clip"
        );

        history.undo(&mut project).expect("undo");
        assert_eq!(project.sequences[0].tracks[0].duration(RATE), frames(48));

        let plan = plan_source_edit(
            &project,
            &sequence,
            media,
            0,
            frames(24),
            EditMode::Overwrite,
        )
        .expect("a plan");
        apply_source_edit(&mut history, &mut project, plan).expect("an overwrite");
        assert_eq!(
            project.sequences[0].tracks[0].duration(RATE),
            frames(48),
            "an overwrite covers the second clip instead of moving it"
        );
    }
}
