//! Dragging a clip's edges: normal trim, and ripple trim.
//!
//! Trimming is the other half of the clip gestures [`crate::selection`] began.
//! A drag on an edge arrives here as one exact offset, and leaves as the
//! ordered commands that offset means: a [`TrimClipIn`] or [`TrimClipOut`] on
//! its own for a normal trim, and the same trim plus a [`MoveClip`] for every
//! clip that has to shift when the trim ripples. The whole list is one entry
//! in the undo stack, so undoing a ripple trim puts the track back as it was
//! rather than unwinding it clip by clip.
//!
//! # Clamping
//!
//! A trim cannot invent frames. The offset a drag asks for is *clamped*, not
//! refused: the edge stops at the end of the source media and at the one-frame
//! minimum a clip has to keep, and the drag carries on harmlessly past it.
//! That is what an editor expects an edge to do when it runs out of handle.
//! Refusals ([`TrimRefusal`]) are for the drags that were never legal at all:
//! a locked track, or a clip that has left the sequence.
//!
//! # Ripple
//!
//! A normal trim leaves a hole or overwrites a neighbour; a ripple trim keeps
//! the track contiguous by shifting the trimmed clip's downstream neighbours
//! by exactly the same amount:
//!
//! - **In point.** The clip's head stays on the timeline instant it was on.
//!   Trimming the in point later by `d` shortens the clip, and the clip itself
//!   and everything after it slide back by `d`.
//! - **Out point.** The clip's head stays put, its tail moves by `d`, and
//!   everything after it slides by `d`.
//!
//! Nothing here touches pixels: a drag reaches [`plan_trim`] already reduced to
//! a [`RationalTime`], so the ghost on screen and the commands committed on
//! release come from the same exact numbers.

use sub_core::{SubError, SubResult};
use sub_edit::clip::{MoveClip, TrimClipIn, TrimClipOut};
use sub_model::{Clip, Project, Sequence};
use sub_time::{Rational, RationalTime, Rounding, TimeRange};

use crate::codes;
use crate::selection::ClipRef;
use crate::timeline_panel::clip_edits_allowed;

/// Which end of a clip a trim drag has hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrimEdge {
    /// The head: the in point moves, the out point stays.
    In,
    /// The tail: the out point moves, the in point stays.
    Out,
}

impl TrimEdge {
    /// A stable identifier, for logging and for tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
        }
    }
}

/// Why a trim drag cannot become an edit at all.
///
/// Running out of source is not here: that is clamped rather than refused (see
/// the module docs). These are the drags that were never legal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimRefusal {
    /// The clip is on a locked track.
    LockedTrack,
    /// The clip is no longer in the sequence.
    UnknownClip,
    /// The arithmetic of the trim overflowed, which no real edit does.
    OutOfRange,
}

impl TrimRefusal {
    /// A stable identifier, for logging and for tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::LockedTrack => "locked_track",
            Self::UnknownClip => "unknown_clip",
            Self::OutOfRange => "out_of_range",
        }
    }

    /// One sentence saying what is wrong, for the error and the tooltip.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::LockedTrack => "a locked track's clips cannot be trimmed",
            Self::UnknownClip => "the trimmed clip is no longer in the sequence",
            Self::OutOfRange => "the trim is too far to represent",
        }
    }

    /// This refusal as a [`SubError`] carrying `ui.clip_trim_refused`.
    #[must_use]
    pub fn to_error(self) -> SubError {
        SubError::new(codes::CLIP_TRIM_REFUSED, self.message()).with_detail("reason", self.id())
    }
}

/// One command a planned trim commits.
///
/// A trim is at most one trim command and any number of moves, and they have
/// to be applied in one order, so they travel as one ordered list rather than
/// as separate fields.
#[derive(Debug, Clone)]
pub enum TrimStep {
    /// Move the clip's in point.
    TrimIn(TrimClipIn),
    /// Move the clip's out point.
    TrimOut(TrimClipOut),
    /// Shift a clip the ripple carried along.
    Move(MoveClip),
}

/// One trim drag reduced to the commands it will commit.
#[derive(Debug, Clone)]
pub struct TrimGroup {
    /// The label the one history entry gets.
    pub label: String,
    /// The clip whose edge is being dragged.
    pub target: ClipRef,
    /// The edge being dragged.
    pub edge: TrimEdge,
    /// Whether later clips move with the trim.
    pub ripple: bool,
    /// How far the edge actually moves, after clamping, at the sequence
    /// timebase.
    pub delta: RationalTime,
    /// The span the trimmed clip occupies afterwards, for the ghost.
    pub range: TimeRange,
    /// How far each rippled clip shifts; zero when nothing ripples.
    pub shift: RationalTime,
    /// The commands, in the order they must be applied.
    pub steps: Vec<TrimStep>,
}

impl TrimGroup {
    /// How many commands the trim commits.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the trim commits nothing, which a planned trim never does.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// How many clips the ripple carries along, the trimmed clip aside.
    #[must_use]
    pub fn rippled(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| matches!(step, TrimStep::Move(_)))
            .count()
    }
}

/// How far an edge may be dragged, at the sequence timebase.
///
/// Both bounds are inclusive and both are always finite: an unprobed source
/// has no known end, so the tail is held at the frames the clip already uses
/// rather than allowed to run off into a file whose length nobody knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bounds {
    lower: RationalTime,
    upper: RationalTime,
}

impl Bounds {
    /// `delta` pulled inside the bounds.
    fn clamp(self, delta: RationalTime) -> RationalTime {
        delta.max(self.lower).min(self.upper)
    }
}

/// Works out what dragging `edge` of `target` by `delta` would do.
///
/// `delta` is a time offset at the sequence timebase, positive to the right on
/// the timeline for both edges. It is clamped to what the clip's source and
/// the one-frame minimum allow; a drag that is clamped to nothing is not an
/// edit and yields `Ok(None)`, so wobbling a pixel inside the handle does not
/// push an undo step. `ripple` shifts the clips after `target` on the same
/// track so the track stays contiguous.
///
/// # Errors
///
/// Returns [`TrimRefusal`] when the drag was never legal: the clip's track is
/// locked, the clip has left the sequence, or the arithmetic overflows.
pub fn plan_trim(
    project: &Project,
    sequence: &Sequence,
    target: ClipRef,
    edge: TrimEdge,
    delta: RationalTime,
    ripple: bool,
) -> Result<Option<TrimGroup>, TrimRefusal> {
    let rate = sequence.settings.frame_rate;
    let track = sequence
        .tracks
        .iter()
        .find(|track| track.id == target.track)
        .ok_or(TrimRefusal::UnknownClip)?;
    if !clip_edits_allowed(track) {
        return Err(TrimRefusal::LockedTrack);
    }
    let (clip, range) = track
        .placements(rate)
        .find_map(|(item, range)| {
            item.as_clip()
                .filter(|clip| clip.id == target.clip)
                .map(|clip| (clip, range))
        })
        .ok_or(TrimRefusal::UnknownClip)?;

    let bounds = bounds_of(project, clip, range, edge, ripple, rate)?;
    let delta = bounds.clamp(delta.rescaled_to(rate));
    if delta.is_zero() {
        return Ok(None);
    }

    let trimmed = trimmed_range(range, edge, delta, ripple)?;
    // A ripple moves the trimmed clip's neighbours by exactly what the edge
    // moved; an in-point ripple also carries the trimmed clip itself back, so
    // its head stays where it was.
    let shift = if ripple {
        match edge {
            TrimEdge::In => delta.checked_neg().ok_or(TrimRefusal::OutOfRange)?,
            TrimEdge::Out => delta,
        }
    } else {
        RationalTime::zero(rate)
    };

    let mut moves = Vec::new();
    if ripple {
        if edge == TrimEdge::In {
            // The trimmed clip is carried too, so its head lands back on the
            // instant it was on. Which side of the trim that move sits on is
            // what the ordering below decides: a leftward ripple trims first
            // and then slides the clip back off its new start, a rightward
            // one slides it out of the way first and lets the trim reach back
            // into the room it left.
            let (from, to) = if shift.is_negative() {
                (
                    range
                        .start()
                        .checked_add(delta)
                        .ok_or(TrimRefusal::OutOfRange)?,
                    range.start(),
                )
            } else {
                (
                    range.start(),
                    range
                        .start()
                        .checked_add(shift)
                        .ok_or(TrimRefusal::OutOfRange)?,
                )
            };
            moves.push((from, to, target.clip));
        }
        for (item, placement) in track.placements(rate) {
            let Some(other) = item.as_clip() else {
                continue;
            };
            if other.id == target.clip || placement.start() < range.end_exclusive() {
                continue;
            }
            let to = placement
                .start()
                .checked_add(shift)
                .ok_or(TrimRefusal::OutOfRange)?;
            if to.is_negative() {
                return Err(TrimRefusal::OutOfRange);
            }
            moves.push((placement.start(), to, other.id));
        }
        // A shift to the right moves the right-most clip first, a shift to the
        // left the left-most, so a clip never overwrites one that has not got
        // out of the way yet.
        if shift.is_negative() {
            moves.sort_by_key(|(from, _, _)| *from);
        } else {
            moves.sort_by_key(|(from, _, _)| core::cmp::Reverse(*from));
        }
    }

    let trim_step = trim_step(sequence, target, edge, delta);
    let move_steps = moves.into_iter().map(|(_, to, clip)| {
        TrimStep::Move(MoveClip {
            sequence: sequence.id,
            track: target.track,
            clip,
            start: to,
            to_track: None,
        })
    });
    // The trim runs first when the ripple pulls the track left, because the
    // clips coming left need the room it gives up; it runs last when the
    // ripple pushes right, because it needs the room they leave.
    let steps: Vec<TrimStep> = if shift.is_negative() {
        core::iter::once(trim_step).chain(move_steps).collect()
    } else {
        move_steps.chain(core::iter::once(trim_step)).collect()
    };

    Ok(Some(TrimGroup {
        label: trim_label(edge, ripple),
        target,
        edge,
        ripple,
        delta,
        range: trimmed,
        shift,
        steps,
    }))
}

/// The command the edge itself commits.
fn trim_step(
    sequence: &Sequence,
    target: ClipRef,
    edge: TrimEdge,
    delta: RationalTime,
) -> TrimStep {
    match edge {
        TrimEdge::In => TrimStep::TrimIn(TrimClipIn {
            sequence: sequence.id,
            track: target.track,
            clip: target.clip,
            delta,
        }),
        TrimEdge::Out => TrimStep::TrimOut(TrimClipOut {
            sequence: sequence.id,
            track: target.track,
            clip: target.clip,
            delta,
        }),
    }
}

/// Where the trimmed clip ends up on the timeline.
fn trimmed_range(
    range: TimeRange,
    edge: TrimEdge,
    delta: RationalTime,
    ripple: bool,
) -> Result<TimeRange, TrimRefusal> {
    let (start, duration) = match edge {
        // A rippled in-point trim keeps the head where it was: the clip slides
        // back by exactly what the in point moved forward.
        TrimEdge::In if ripple => (
            range.start(),
            range
                .duration()
                .checked_sub(delta)
                .ok_or(TrimRefusal::OutOfRange)?,
        ),
        TrimEdge::In => (
            range
                .start()
                .checked_add(delta)
                .ok_or(TrimRefusal::OutOfRange)?,
            range
                .duration()
                .checked_sub(delta)
                .ok_or(TrimRefusal::OutOfRange)?,
        ),
        TrimEdge::Out => (
            range.start(),
            range
                .duration()
                .checked_add(delta)
                .ok_or(TrimRefusal::OutOfRange)?,
        ),
    };
    TimeRange::new(start, duration).ok_or(TrimRefusal::OutOfRange)
}

/// How far the edge may be dragged in each direction.
fn bounds_of(
    project: &Project,
    clip: &Clip,
    range: TimeRange,
    edge: TrimEdge,
    ripple: bool,
    rate: Rational,
) -> Result<Bounds, TrimRefusal> {
    let frame = RationalTime::new(1, rate);
    // Whatever else a trim does, it leaves the clip at least one frame long.
    let room = range
        .duration()
        .checked_sub(frame)
        .ok_or(TrimRefusal::OutOfRange)?;
    let source = clip.source_range;
    Ok(match edge {
        TrimEdge::In => {
            // The head cannot reach before the first frame of the source, and
            // a normal trim cannot drag it before the head of the sequence
            // either; a rippled one never moves the head at all.
            let mut lower = negated(bound(source.start(), rate, Rounding::Floor))?;
            if !ripple {
                lower = lower.max(negated(range.start())?);
            }
            Bounds { lower, upper: room }
        }
        TrimEdge::Out => {
            // The tail stops at the end of the probed source; an unprobed one
            // has no known end, so the tail is held where it is.
            let limit = project
                .media_item(clip.media)
                .and_then(|item| item.info.as_ref())
                .and_then(|info| info.duration);
            let upper = match limit {
                Some(limit) => bound(limit, rate, Rounding::Floor)
                    .checked_sub(bound(source.end_exclusive(), rate, Rounding::Ceil))
                    .ok_or(TrimRefusal::OutOfRange)?
                    .max(RationalTime::zero(rate)),
                None => RationalTime::zero(rate),
            };
            Bounds {
                lower: negated(room)?,
                upper,
            }
        }
    })
}

/// `time` at the sequence timebase, rounded the way that keeps a bound safe.
fn bound(time: RationalTime, rate: Rational, rounding: Rounding) -> RationalTime {
    time.rescaled_to_rounding(rate, rounding)
}

/// `-time`, or [`TrimRefusal::OutOfRange`].
fn negated(time: RationalTime) -> Result<RationalTime, TrimRefusal> {
    time.checked_neg().ok_or(TrimRefusal::OutOfRange)
}

/// The history label a trim gets.
fn trim_label(edge: TrimEdge, ripple: bool) -> String {
    match (edge, ripple) {
        (TrimEdge::In, false) => "Trim clip in".to_owned(),
        (TrimEdge::Out, false) => "Trim clip out".to_owned(),
        (TrimEdge::In, true) => "Ripple trim clip in".to_owned(),
        (TrimEdge::Out, true) => "Ripple trim clip out".to_owned(),
    }
}

/// Applies a planned trim as one undoable step.
///
/// The trim and every move the ripple carried go into a single
/// [`History`](sub_edit::History) entry, so one undo puts the whole track
/// back. A command that fails takes the group with it.
///
/// # Errors
///
/// Whatever the trim or a move returns, and `edit.group_open` when a group is
/// already open.
pub fn apply_trim(
    history: &mut sub_edit::History,
    project: &mut Project,
    group: &TrimGroup,
) -> SubResult<()> {
    history.begin_group(group.label.clone())?;
    for step in &group.steps {
        match step {
            TrimStep::TrimIn(command) => history.apply(project, command.clone())?,
            TrimStep::TrimOut(command) => history.apply(project, command.clone())?,
            TrimStep::Move(command) => history.apply(project, command.clone())?,
        }
    }
    history.commit_group()?;
    Ok(())
}

/// Turns a refusal into the error the Command API would have raised.
///
/// # Errors
///
/// Always: it is a refusal.
pub fn refuse<T>(refusal: TrimRefusal) -> SubResult<T> {
    Err(refusal.to_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_model::media::StreamInfo;
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Gap, MediaItem, MediaPath, Track, TrackItem, TrackKind};

    const RATE: Rational = Rational::FPS_24;
    /// The source every clip in the scene is cut from, in frames.
    const SOURCE_FRAMES: i64 = 240;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, RATE)
    }

    fn range(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(frames(start), frames(duration)).expect("a valid range")
    }

    /// A project whose V1 holds `head` at 0..48 and `tail` at 96..144, both cut
    /// from source 24..72 of a 240-frame probed file, so both have handle at
    /// each end.
    fn scene() -> (Project, Sequence) {
        let mut project = Project::new("trim");
        let mut item = MediaItem::new(MediaPath::new("media/take.mp4").expect("a valid path"));
        item.info = Some(StreamInfo {
            duration: Some(frames(SOURCE_FRAMES)),
            video: Vec::new(),
            audio: Vec::new(),
        });
        let media = item.id;
        project.media.push(item);

        let mut sequence = Sequence::new("edit", SequenceSettings::default());
        let mut track = Track::new("V1", TrackKind::Video);
        track
            .items
            .push(TrackItem::Clip(Clip::new("head", media, range(24, 48))));
        track.items.push(TrackItem::Gap(Gap::new(frames(48))));
        track
            .items
            .push(TrackItem::Clip(Clip::new("tail", media, range(24, 48))));
        sequence.tracks.push(track);
        sequence.tracks.push(Track::new("V2", TrackKind::Video));
        project.sequences.push(sequence.clone());
        (project, sequence)
    }

    fn clip_ref(sequence: &Sequence, track: usize, clip: usize) -> ClipRef {
        let track = &sequence.tracks[track];
        let id = track
            .items
            .iter()
            .filter_map(TrackItem::as_clip)
            .nth(clip)
            .expect("the clip exists")
            .id;
        ClipRef::new(track.id, id)
    }

    /// Every clip's span on track 0 of the project's sequence, in frames.
    fn spans(project: &Project) -> Vec<(i64, i64)> {
        project.sequences[0].tracks[0]
            .placements(RATE)
            .filter(|(item, _)| item.as_clip().is_some())
            .map(|(_, range)| (range.start().value(), range.duration().value()))
            .collect()
    }

    fn plan(
        project: &Project,
        sequence: &Sequence,
        clip: usize,
        edge: TrimEdge,
        delta: i64,
        ripple: bool,
    ) -> TrimGroup {
        plan_trim(
            project,
            sequence,
            clip_ref(sequence, 0, clip),
            edge,
            frames(delta),
            ripple,
        )
        .expect("a legal trim")
        .expect("a trim that moved")
    }

    #[test]
    fn dragging_the_out_point_lengthens_the_clip() {
        let (project, sequence) = scene();
        let group = plan(&project, &sequence, 0, TrimEdge::Out, 12, false);
        assert_eq!(group.label, "Trim clip out");
        assert_eq!(group.delta, frames(12));
        assert_eq!(group.range, range(0, 60));
        assert_eq!(group.len(), 1, "a normal trim is one command");
        assert_eq!(group.rippled(), 0);
        assert!(matches!(group.steps[0], TrimStep::TrimOut(_)));
    }

    #[test]
    fn dragging_the_in_point_moves_the_head_and_keeps_the_tail() {
        let (project, sequence) = scene();
        let group = plan(&project, &sequence, 0, TrimEdge::In, 12, false);
        assert_eq!(group.label, "Trim clip in");
        assert_eq!(
            group.range,
            range(12, 36),
            "the head moves right and the out point stays"
        );
        assert!(matches!(group.steps[0], TrimStep::TrimIn(_)));
    }

    #[test]
    fn a_trim_is_clamped_to_the_source_rather_than_refused() {
        let (project, sequence) = scene();

        // `head` uses source 24..72 of a 240-frame file, so the tail has 168
        // frames of handle left and no more.
        let group = plan(&project, &sequence, 0, TrimEdge::Out, 1_000, false);
        assert_eq!(group.delta, frames(SOURCE_FRAMES - 72));
        assert_eq!(group.range, range(0, 48 + SOURCE_FRAMES - 72));

        // And `tail`, which sits at 96 rather than at the head of the
        // sequence, has 24 frames of handle before its source runs out.
        let group = plan(&project, &sequence, 1, TrimEdge::In, -1_000, false);
        assert_eq!(group.delta, frames(-24));
        assert_eq!(
            group.range,
            range(72, 72),
            "clamped to the head of the media"
        );
    }

    #[test]
    fn a_trim_leaves_the_clip_at_least_one_frame_long() {
        let (project, sequence) = scene();
        let group = plan(&project, &sequence, 0, TrimEdge::In, 1_000, false);
        assert_eq!(group.delta, frames(47));
        assert_eq!(group.range, range(47, 1));

        let group = plan(&project, &sequence, 0, TrimEdge::Out, -1_000, false);
        assert_eq!(group.delta, frames(-47));
        assert_eq!(group.range, range(0, 1));
    }

    #[test]
    fn a_normal_in_trim_cannot_reach_before_the_head_of_the_sequence() {
        let (project, sequence) = scene();
        // `head` starts at sequence zero, so its in point has nowhere to go
        // left even though the source has handle.
        assert!(
            plan_trim(
                &project,
                &sequence,
                clip_ref(&sequence, 0, 0),
                TrimEdge::In,
                frames(-12),
                false,
            )
            .expect("a legal trim")
            .is_none(),
            "clamped to nothing, so not an edit"
        );
    }

    #[test]
    fn an_unprobed_source_holds_the_out_point_where_it_is() {
        let (mut project, sequence) = scene();
        project.media[0].info = None;
        assert!(
            plan_trim(
                &project,
                &sequence,
                clip_ref(&sequence, 0, 0),
                TrimEdge::Out,
                frames(12),
                false,
            )
            .expect("a legal trim")
            .is_none(),
            "nobody knows how long the file is, so the tail does not grow"
        );
    }

    #[test]
    fn a_ripple_out_trim_carries_the_later_clips_with_it() {
        let (project, sequence) = scene();
        let group = plan(&project, &sequence, 0, TrimEdge::Out, 12, true);
        assert_eq!(group.label, "Ripple trim clip out");
        assert_eq!(group.shift, frames(12));
        assert_eq!(group.rippled(), 1, "the clip after it moves");
        assert!(
            matches!(group.steps[0], TrimStep::Move(_)),
            "the neighbour gets out of the way before the trim needs the room"
        );
        assert!(matches!(group.steps[1], TrimStep::TrimOut(_)));
    }

    #[test]
    fn a_ripple_in_trim_keeps_the_head_and_pulls_the_track_back() {
        let (project, sequence) = scene();
        let group = plan(&project, &sequence, 0, TrimEdge::In, 12, true);
        assert_eq!(group.shift, frames(-12));
        assert_eq!(
            group.range,
            range(0, 36),
            "the head stays put and the clip gets shorter"
        );
        assert_eq!(
            group.rippled(),
            2,
            "the clip itself slides back, and so does its neighbour"
        );
        assert!(
            matches!(group.steps[0], TrimStep::TrimIn(_)),
            "the trim gives up the room the clips coming left need"
        );
    }

    #[test]
    fn a_ripple_trim_applies_as_one_undo_step() {
        let (mut project, sequence) = scene();
        let group = plan(&project, &sequence, 0, TrimEdge::Out, 12, true);
        let mut history = sub_edit::History::new();
        apply_trim(&mut history, &mut project, &group).expect("the group applies");
        assert_eq!(history.undo_len(), 1, "a trim drag is one step");
        assert_eq!(
            spans(&project),
            vec![(0, 60), (108, 48)],
            "the clip grew and its neighbour moved by the same amount"
        );

        history.undo(&mut project).expect("undo works");
        assert_eq!(
            spans(&project),
            vec![(0, 48), (96, 48)],
            "one undo puts the whole track back"
        );
    }

    #[test]
    fn a_ripple_in_trim_applies_as_one_undo_step() {
        let (mut project, sequence) = scene();
        let group = plan(&project, &sequence, 0, TrimEdge::In, 12, true);
        let mut history = sub_edit::History::new();
        apply_trim(&mut history, &mut project, &group).expect("the group applies");
        assert_eq!(
            spans(&project),
            vec![(0, 36), (84, 48)],
            "the head stayed, the clip shortened and the track closed up"
        );
        history.undo(&mut project).expect("undo works");
        assert_eq!(spans(&project), vec![(0, 48), (96, 48)]);
    }

    #[test]
    fn a_normal_trim_applies_without_moving_anything_else() {
        let (mut project, sequence) = scene();
        let group = plan(&project, &sequence, 0, TrimEdge::Out, 12, false);
        let mut history = sub_edit::History::new();
        apply_trim(&mut history, &mut project, &group).expect("the group applies");
        assert_eq!(
            spans(&project),
            vec![(0, 60), (96, 48)],
            "the neighbour stayed where it was"
        );
    }

    #[test]
    fn a_locked_track_and_a_missing_clip_are_refused() {
        let (project, mut sequence) = scene();
        let target = clip_ref(&sequence, 0, 0);
        sequence.tracks[0].locked = true;
        assert_eq!(
            plan_trim(
                &project,
                &sequence,
                target,
                TrimEdge::Out,
                frames(12),
                false
            )
            .unwrap_err(),
            TrimRefusal::LockedTrack
        );

        sequence.tracks[0].locked = false;
        sequence.tracks[0].items.clear();
        assert_eq!(
            plan_trim(
                &project,
                &sequence,
                target,
                TrimEdge::Out,
                frames(12),
                false
            )
            .unwrap_err(),
            TrimRefusal::UnknownClip
        );
    }

    #[test]
    fn every_refusal_has_a_stable_id() {
        for refusal in [
            TrimRefusal::LockedTrack,
            TrimRefusal::UnknownClip,
            TrimRefusal::OutOfRange,
        ] {
            assert!(!refusal.id().is_empty());
            assert!(!refusal.message().is_empty());
            assert_eq!(refusal.to_error().code, codes::CLIP_TRIM_REFUSED);
        }
        assert!(refuse::<()>(TrimRefusal::UnknownClip).is_err());
        assert_eq!(TrimEdge::In.id(), "in");
        assert_eq!(TrimEdge::Out.id(), "out");
    }
}
