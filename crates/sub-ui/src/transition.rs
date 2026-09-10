//! Dragging a crossfade's duration on the timeline.
//!
//! A crossfade is drawn as the region it blends across, centred on the cut
//! (docs/PLAN.md §5.3). Dragging either edge of that region outwards makes the
//! blend longer and inwards makes it shorter, symmetrically: the cut itself
//! never moves, so a drag of `d` points on one edge changes the duration by
//! twice the time those points cover.
//!
//! Like the trim gestures in [`crate::trim`], nothing here touches pixels. A
//! drag arrives already reduced to an exact [`RationalTime`], leaves as the one
//! [`AddTransition`] command that expresses it, and the clamping to the
//! available handles is the command's own — the panel asks
//! [`sub_edit::commands::fit_crossfade`] what the drag would really become so
//! the region it paints and the command it commits agree frame for frame.

use sub_core::{SubError, SubResult};
use sub_edit::commands::AddTransition;
use sub_model::{ClipId, Project, Sequence, TrackId, Transition};
use sub_time::{Rational, RationalTime};

use crate::codes;
use crate::timeline_panel::clip_edits_allowed;

/// The crossfade a drag has hold of: the cut is named by the clip the blend
/// fades into, exactly as the commands name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransitionRef {
    /// The track holding the cut.
    pub track: TrackId,
    /// The clip the blend fades into.
    pub clip: ClipId,
}

impl TransitionRef {
    /// The crossfade at the cut before `clip` on `track`.
    #[must_use]
    pub const fn new(track: TrackId, clip: ClipId) -> Self {
        Self { track, clip }
    }
}

/// Which edge of the blend region a drag has hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransitionEdge {
    /// The head, before the cut: dragging it left lengthens the blend.
    Head,
    /// The tail, after the cut: dragging it right lengthens the blend.
    Tail,
}

impl TransitionEdge {
    /// A stable identifier, for logging and for tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Tail => "tail",
        }
    }

    /// How a drag of `delta` on this edge changes the duration: outwards is
    /// longer, and both halves move, so the change is twice the drag.
    #[must_use]
    pub fn duration_delta(self, delta: RationalTime) -> Option<RationalTime> {
        let signed = match self {
            Self::Head => delta.checked_neg()?,
            Self::Tail => delta,
        };
        signed.checked_add(signed)
    }
}

/// Why a crossfade drag cannot become an edit at all.
///
/// Running out of handle is not here: that is clamped by the command rather
/// than refused, exactly as a trim's overrun is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionRefusal {
    /// The cut is on a locked track.
    LockedTrack,
    /// The crossfade is no longer in the sequence.
    UnknownTransition,
    /// The drag would leave nothing of the blend at all.
    TooShort,
    /// The arithmetic of the drag overflowed, which no real edit does.
    OutOfRange,
}

impl TransitionRefusal {
    /// A stable identifier, for logging and for tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::LockedTrack => "locked_track",
            Self::UnknownTransition => "unknown_transition",
            Self::TooShort => "too_short",
            Self::OutOfRange => "out_of_range",
        }
    }

    /// One sentence saying what is wrong, for the error and the tooltip.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::LockedTrack => "a locked track's transitions cannot be edited",
            Self::UnknownTransition => "the transition is no longer in the sequence",
            Self::TooShort => "a transition lasts more than no time at all",
            Self::OutOfRange => "the transition is too long to represent",
        }
    }

    /// This refusal as a [`SubError`] carrying `ui.transition_refused`.
    #[must_use]
    pub fn to_error(self) -> SubError {
        SubError::new(codes::TRANSITION_REFUSED, self.message()).with_detail("reason", self.id())
    }
}

/// What a crossfade drag would commit: the command, and the blend it produces
/// once the command's own clamping has been applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionDrag {
    /// The crossfade being dragged.
    pub target: TransitionRef,
    /// The command that commits the drag.
    pub command: AddTransition,
    /// The blend the command really produces, for the region to paint.
    pub fitted: Transition,
}

impl TransitionDrag {
    /// The label the undo stack shows for this edit.
    #[must_use]
    pub fn label(&self) -> String {
        "Change crossfade duration".to_owned()
    }
}

/// Plans the crossfade a drag of `delta` on `edge` of `target` asks for.
///
/// The duration is read off the transition as it is now and moved by twice the
/// drag, so the two edges stay symmetric about the cut; the command clamps
/// what is left to the handles either side.
///
/// # Errors
///
/// Returns a [`TransitionRefusal`] when the drag was never legal: a locked
/// track, a transition that has left the sequence, arithmetic that will not
/// represent, or a drag that has closed the blend completely.
pub fn plan_transition(
    project: &Project,
    sequence: &Sequence,
    target: TransitionRef,
    delta: RationalTime,
) -> Result<TransitionDrag, TransitionRefusal> {
    let rate = sequence.settings.frame_rate;
    let track = sequence
        .track(target.track)
        .ok_or(TransitionRefusal::UnknownTransition)?;
    if !clip_edits_allowed(track) {
        return Err(TransitionRefusal::LockedTrack);
    }
    let current = current_duration(track, target.clip, rate)
        .ok_or(TransitionRefusal::UnknownTransition)?
        .checked_rescaled_to(rate)
        .ok_or(TransitionRefusal::OutOfRange)?;
    let wanted = current
        .checked_add(delta)
        .and_then(|wanted| wanted.checked_rescaled_to(rate))
        .ok_or(TransitionRefusal::OutOfRange)?;
    if wanted.is_negative() || wanted.is_zero() {
        return Err(TransitionRefusal::TooShort);
    }
    let command = AddTransition::new(sequence.id, target.track, target.clip, wanted);
    let fitted =
        sub_edit::commands::fit_crossfade(project, sequence.id, target.track, target.clip, wanted)
            .map_err(|_| TransitionRefusal::UnknownTransition)?;
    Ok(TransitionDrag {
        target,
        command,
        fitted,
    })
}

/// How long the crossfade at the cut before `clip` is now.
fn current_duration(
    track: &sub_model::Track,
    clip: ClipId,
    rate: Rational,
) -> Option<RationalTime> {
    let index = track
        .items
        .iter()
        .position(|item| item.as_clip().is_some_and(|found| found.id == clip))?;
    let transition = track.items.get(index.checked_sub(1)?)?.as_transition()?;
    transition.duration().checked_rescaled_to(rate)
}

/// Applies a planned crossfade drag through `history`.
///
/// One command, so one entry in the undo stack: undoing puts the previous
/// offsets back exactly, a transition that was not there at all included.
///
/// # Errors
///
/// Whatever [`AddTransition`] raises: a locked track, a cut that has gone, or
/// a cut with no handle left to blend across.
pub fn apply_transition(
    history: &mut sub_edit::History,
    project: &mut Project,
    drag: &TransitionDrag,
) -> SubResult<()> {
    history.apply(project, drag.command)?;
    Ok(())
}

/// Turns a refusal into the error the Command API would have raised.
///
/// # Errors
///
/// Always: it is a refusal.
pub fn refuse<T>(refusal: TransitionRefusal) -> SubResult<T> {
    Err(refusal.to_error())
}

#[cfg(test)]
mod tests {
    use sub_model::media::StreamInfo;
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Clip, MediaItem, MediaPath, Track, TrackItem, TrackKind};
    use sub_time::TimeRange;

    use super::*;

    const RATE: Rational = Rational::FPS_24;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, RATE)
    }

    fn range(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(frames(start), frames(duration)).expect("a valid range")
    }

    /// A project whose V1 holds two butt-joined 48-frame clips with a
    /// 12-frame crossfade between them, both cut out of a probed 480-frame
    /// file so there is handle on both sides.
    fn scene() -> (Project, Sequence, TransitionRef) {
        let mut project = Project::new("crossfade");
        let mut item = MediaItem::new(MediaPath::new("media/take.mp4").expect("a valid path"));
        item.info = Some(StreamInfo {
            duration: Some(frames(480)),
            video: Vec::new(),
            audio: Vec::new(),
        });
        let media = item.id;
        project.media.push(item);

        let outgoing = Clip::new("a", media, range(48, 48));
        let incoming = Clip::new("b", media, range(240, 48));
        let incoming_id = incoming.id;
        let mut track = Track::new("V1", TrackKind::Video);
        track.items.push(outgoing.into());
        track
            .items
            .push(TrackItem::Transition(Transition::crossfade(
                frames(6),
                frames(6),
            )));
        track.items.push(incoming.into());
        let track_id = track.id;
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.tracks.push(track);
        project.sequences.push(sequence.clone());
        (project, sequence, TransitionRef::new(track_id, incoming_id))
    }

    #[test]
    fn dragging_the_tail_out_lengthens_the_blend_by_twice_the_drag() {
        let (project, sequence, target) = scene();
        let delta = TransitionEdge::Tail
            .duration_delta(frames(4))
            .expect("in range");
        let drag = plan_transition(&project, &sequence, target, delta).expect("a plan");
        assert_eq!(drag.command.duration, frames(20));
        assert_eq!(drag.fitted.duration(), frames(20));
        assert_eq!(drag.fitted.in_offset(), frames(10));
    }

    #[test]
    fn dragging_the_head_out_lengthens_it_too() {
        let (project, sequence, target) = scene();
        let delta = TransitionEdge::Head
            .duration_delta(frames(-4))
            .expect("in range");
        let drag = plan_transition(&project, &sequence, target, delta).expect("a plan");
        assert_eq!(drag.command.duration, frames(20));
    }

    #[test]
    fn dragging_an_edge_inwards_shortens_it() {
        let (project, sequence, target) = scene();
        let delta = TransitionEdge::Tail
            .duration_delta(frames(-2))
            .expect("in range");
        let drag = plan_transition(&project, &sequence, target, delta).expect("a plan");
        assert_eq!(drag.command.duration, frames(8));
    }

    #[test]
    fn closing_the_blend_completely_is_refused() {
        let (project, sequence, target) = scene();
        let delta = TransitionEdge::Tail
            .duration_delta(frames(-20))
            .expect("in range");
        let refusal =
            plan_transition(&project, &sequence, target, delta).expect_err("nothing is left");
        assert_eq!(refusal, TransitionRefusal::TooShort);
        assert_eq!(
            refuse::<()>(refusal).unwrap_err().code,
            codes::TRANSITION_REFUSED
        );
    }

    #[test]
    fn a_locked_track_refuses_the_drag() {
        let (project, mut sequence, target) = scene();
        sequence.tracks[0].locked = true;
        let refusal = plan_transition(&project, &sequence, target, frames(4))
            .expect_err("a locked track edits nothing");
        assert_eq!(refusal, TransitionRefusal::LockedTrack);
    }

    #[test]
    fn a_clip_with_no_transition_is_unknown() {
        let (project, sequence, target) = scene();
        let other = sequence.tracks[0].items[0].as_clip().expect("a clip").id;
        let refusal = plan_transition(
            &project,
            &sequence,
            TransitionRef::new(target.track, other),
            frames(4),
        )
        .expect_err("no transition there");
        assert_eq!(refusal, TransitionRefusal::UnknownTransition);
    }

    #[test]
    fn the_plan_applies_as_one_undoable_command() {
        let (mut project, sequence, target) = scene();
        let drag = plan_transition(&project, &sequence, target, frames(12)).expect("a plan");
        let mut history = sub_edit::History::new();
        apply_transition(&mut history, &mut project, &drag).expect("it applies");
        let duration = project.sequences[0].tracks[0].items[1]
            .as_transition()
            .expect("a crossfade")
            .duration();
        assert_eq!(duration, frames(24));

        history.undo(&mut project).expect("undo").expect("a step");
        let duration = project.sequences[0].tracks[0].items[1]
            .as_transition()
            .expect("a crossfade")
            .duration();
        assert_eq!(duration, frames(12), "undo puts the old blend back");
    }
}
