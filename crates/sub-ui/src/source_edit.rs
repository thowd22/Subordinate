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
use sub_edit::{BoxedCommand, History};
use sub_model::{Clip, MediaId, MediaItem, Project, Sequence, SequenceId, TrackId, TrackKind};
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

    /// The command this edit is, ready for the Command API.
    #[must_use]
    pub fn into_command(self) -> BoxedCommand {
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
    })
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
    history.apply_boxed(project, plan.into_command())?;
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
