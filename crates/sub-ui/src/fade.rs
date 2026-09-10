//! Dragging an audio clip's fade handles.
//!
//! A fade is a duration on the clip ([`sub_model::Clip::fade_in`] and
//! [`sub_model::Clip::fade_out`]), so dragging a handle is one
//! [`SetClipParams`] and nothing else. The timeline hands the drag over as an
//! exact offset and gets back the command it means, which is what keeps the
//! ramp painted under the pointer and the value written into the project the
//! same number.
//!
//! # Clamping
//!
//! A fade cannot be longer than what is left of the clip beside the other
//! fade, and it cannot be negative. Both ends are *clamped* rather than
//! refused, so a handle dragged past the far end of the clip stops there and
//! the drag carries on harmlessly. Refusals ([`FadeRefusal`]) are for the
//! drags that were never legal at all: a locked track, or a clip that has left
//! the sequence.
//!
//! Nothing here touches pixels: a drag arrives already reduced to a
//! [`RationalTime`] at the sequence timebase.

use sub_core::{SubError, SubResult};
use sub_edit::commands::SetClipParams;
use sub_model::{Project, Sequence};
use sub_time::{RationalTime, TimeRange};

use crate::codes;
use crate::selection::ClipRef;
use crate::timeline_panel::clip_edits_allowed;

/// Which of a clip's two fades a drag has hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FadeEdge {
    /// The head: the clip ramps up from nothing.
    In,
    /// The tail: the clip ramps down to nothing.
    Out,
}

impl FadeEdge {
    /// A stable identifier, for logging and for tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
        }
    }

    /// The history label a drag on this handle produces.
    #[must_use]
    pub const fn undo_label(self) -> &'static str {
        match self {
            Self::In => "Change fade in",
            Self::Out => "Change fade out",
        }
    }
}

/// Why a fade drag cannot become an edit at all.
///
/// Running out of clip is not here: that is clamped rather than refused (see
/// the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeRefusal {
    /// The clip is on a locked track.
    LockedTrack,
    /// The clip is no longer in the sequence.
    UnknownClip,
    /// The arithmetic of the fade overflowed, which no real edit does.
    OutOfRange,
}

impl FadeRefusal {
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
            Self::LockedTrack => "a locked track's clips cannot be faded",
            Self::UnknownClip => "the faded clip is no longer in the sequence",
            Self::OutOfRange => "the fade is too long to represent",
        }
    }

    /// This refusal as a [`SubError`] carrying `ui.clip_fade_refused`.
    #[must_use]
    pub fn to_error(self) -> SubError {
        SubError::new(codes::CLIP_FADE_REFUSED, self.message()).with_detail("reason", self.id())
    }
}

/// One fade drag reduced to the command it will commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FadeEdit {
    /// The clip whose handle is being dragged.
    pub target: ClipRef,
    /// The handle being dragged.
    pub edge: FadeEdge,
    /// How long the fade is afterwards, at the sequence timebase.
    pub duration: RationalTime,
    /// The span the clip occupies, for painting the ramp.
    pub range: TimeRange,
    /// The command the drag commits.
    pub command: SetClipParams,
}

impl FadeEdit {
    /// The label the history entry gets.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        self.edge.undo_label()
    }
}

/// Works out what dragging `edge` of `target` by `delta` would do.
///
/// `delta` is a time offset at the sequence timebase, positive to the right on
/// the timeline for both handles: dragging right lengthens a fade in and
/// shortens a fade out, which is what the handle under the pointer does. The
/// result is clamped to the clip, and a drag that changes nothing yields
/// `Ok(None)` rather than an edit, so wobbling a pixel inside the handle does
/// not push an undo step.
///
/// # Errors
///
/// Returns [`FadeRefusal`] when the drag was never legal: the clip's track is
/// locked, the clip has left the sequence, or the arithmetic overflows.
pub fn plan_fade(
    sequence: &Sequence,
    target: ClipRef,
    edge: FadeEdge,
    delta: RationalTime,
) -> Result<Option<FadeEdit>, FadeRefusal> {
    let rate = sequence.settings.frame_rate;
    let track = sequence
        .tracks
        .iter()
        .find(|track| track.id == target.track)
        .ok_or(FadeRefusal::UnknownClip)?;
    if !clip_edits_allowed(track) {
        return Err(FadeRefusal::LockedTrack);
    }
    let (clip, range) = track
        .placements(rate)
        .find_map(|(item, range)| {
            item.as_clip()
                .filter(|clip| clip.id == target.clip)
                .map(|clip| (clip, range))
        })
        .ok_or(FadeRefusal::UnknownClip)?;

    let zero = RationalTime::zero(rate);
    let delta = delta.rescaled_to(rate);
    let current = fade_of(clip, edge).rescaled_to(rate);
    let other = fade_of(clip, edge.other()).rescaled_to(rate);
    let wanted = match edge {
        FadeEdge::In => current.checked_add(delta),
        FadeEdge::Out => current.checked_sub(delta),
    }
    .ok_or(FadeRefusal::OutOfRange)?;
    // What is left of the clip beside the other fade is all the room there
    // is: the model refuses two fades that together outlast the clip.
    let room = range
        .duration()
        .rescaled_to(rate)
        .checked_sub(other)
        .ok_or(FadeRefusal::OutOfRange)?
        .max(zero);
    let duration = wanted.max(zero).min(room);
    if duration == current {
        return Ok(None);
    }

    let command = SetClipParams::new(sequence.id, target.track, target.clip);
    let command = match edge {
        FadeEdge::In => command.with_fade_in(duration),
        FadeEdge::Out => command.with_fade_out(duration),
    };
    Ok(Some(FadeEdit {
        target,
        edge,
        duration,
        range,
        command,
    }))
}

/// Applies a planned fade as one entry in the undo stack.
///
/// # Errors
///
/// Whatever [`SetClipParams`] returns: a fade the clip has no room for is
/// `model.invalid_parameter`, though a planned fade is clamped and so never
/// is.
pub fn apply_fade(
    history: &mut sub_edit::History,
    project: &mut Project,
    edit: &FadeEdit,
) -> SubResult<()> {
    history.apply(project, edit.command)?;
    Ok(())
}

/// Turns a refusal into the error the Command API would have raised.
///
/// # Errors
///
/// Always: it is a refusal.
pub fn refuse<T>(refusal: FadeRefusal) -> SubResult<T> {
    Err(refusal.to_error())
}

impl FadeEdge {
    /// The other handle of the same clip.
    const fn other(self) -> Self {
        match self {
            Self::In => Self::Out,
            Self::Out => Self::In,
        }
    }
}

/// The length of one of `clip`'s fades.
fn fade_of(clip: &sub_model::Clip, edge: FadeEdge) -> RationalTime {
    match edge {
        FadeEdge::In => clip.fade_in,
        FadeEdge::Out => clip.fade_out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_edit::History;
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Clip, MediaItem, MediaPath, Track, TrackItem, TrackKind};
    use sub_time::Rational;

    /// The timebase every test here works at.
    const RATE: Rational = Rational::FPS_24;

    /// `frames` frames at the test timebase.
    fn frames(count: i64) -> RationalTime {
        RationalTime::new(count, RATE)
    }

    /// A project holding one audio track with one 48 frame clip.
    fn scene() -> (Project, Sequence, ClipRef) {
        let mut project = Project::new("fades");
        let item = MediaItem::new(MediaPath::new("media/take.wav").expect("valid path"));
        let media = item.id;
        project.media.push(item);

        let source = TimeRange::new(frames(0), frames(48)).expect("valid source range");
        let clip = Clip::new("take", media, source);
        let target_clip = clip.id;
        let mut track = Track::new("A1", TrackKind::Audio);
        track.items.push(TrackItem::Clip(clip));
        let target = ClipRef::new(track.id, target_clip);

        let mut sequence = Sequence::new("edit", SequenceSettings::default());
        sequence.tracks.push(track);
        project.sequences.push(sequence.clone());
        (project, sequence, target)
    }

    #[test]
    fn dragging_the_head_handle_right_lengthens_the_fade_in() {
        let (_, sequence, target) = scene();
        let edit = plan_fade(&sequence, target, FadeEdge::In, frames(12))
            .expect("the drag is legal")
            .expect("the drag is an edit");
        assert_eq!(edit.duration, frames(12));
        assert_eq!(edit.command.fade_in, Some(frames(12)));
        assert_eq!(edit.command.fade_out, None);
        assert_eq!(edit.label(), "Change fade in");
    }

    #[test]
    fn dragging_the_tail_handle_left_lengthens_the_fade_out() {
        let (_, sequence, target) = scene();
        let edit = plan_fade(&sequence, target, FadeEdge::Out, frames(-8))
            .expect("the drag is legal")
            .expect("the drag is an edit");
        assert_eq!(edit.duration, frames(8));
        assert_eq!(edit.command.fade_out, Some(frames(8)));
    }

    #[test]
    fn a_fade_is_clamped_to_the_clip_and_never_goes_negative() {
        let (_, sequence, target) = scene();
        let long = plan_fade(&sequence, target, FadeEdge::In, frames(600))
            .expect("the drag is legal")
            .expect("the drag is an edit");
        assert_eq!(
            long.duration,
            frames(48),
            "the fade stops at the clip's end"
        );

        let short =
            plan_fade(&sequence, target, FadeEdge::In, frames(-600)).expect("the drag is legal");
        assert!(
            short.is_none(),
            "a fade already at zero cannot be shortened"
        );
    }

    #[test]
    fn a_fade_leaves_room_for_the_one_at_the_other_end() {
        let (mut project, mut sequence, target) = scene();
        let mut history = History::new();
        let planned = plan_fade(&sequence, target, FadeEdge::Out, frames(-30))
            .expect("the drag is legal")
            .expect("the drag is an edit");
        apply_fade(&mut history, &mut project, &planned).expect("the fade applies");
        sequence = project.sequences[0].clone();

        let edit = plan_fade(&sequence, target, FadeEdge::In, frames(48))
            .expect("the drag is legal")
            .expect("the drag is an edit");
        assert_eq!(
            edit.duration,
            frames(18),
            "the head fade stops where the tail fade starts"
        );

        apply_fade(&mut history, &mut project, &edit).expect("the clamped fade applies");
        let clip = project.sequences[0].tracks[0]
            .clip(target.clip)
            .expect("the clip is still there");
        assert_eq!(clip.fade_in, frames(18));
        assert_eq!(clip.fade_out, frames(30));

        history.undo(&mut project).expect("the fade undoes");
        let clip = project.sequences[0].tracks[0]
            .clip(target.clip)
            .expect("the clip is still there");
        assert!(clip.fade_in.is_zero(), "undo puts the head fade back");
        assert_eq!(clip.fade_out, frames(30), "and leaves the tail one alone");
    }

    #[test]
    fn a_locked_track_refuses_the_drag() {
        let (_, mut sequence, target) = scene();
        sequence.tracks[0].locked = true;
        let refusal = plan_fade(&sequence, target, FadeEdge::In, frames(12))
            .expect_err("a locked track refuses");
        assert_eq!(refusal, FadeRefusal::LockedTrack);
        assert_eq!(refusal.id(), "locked_track");
        let error = refusal.to_error();
        assert_eq!(error.code, codes::CLIP_FADE_REFUSED);
        assert!(refuse::<()>(refusal).is_err());
    }

    #[test]
    fn a_clip_that_has_left_the_sequence_refuses_the_drag() {
        let (_, mut sequence, target) = scene();
        sequence.tracks[0].items.clear();
        assert_eq!(
            plan_fade(&sequence, target, FadeEdge::In, frames(12)).expect_err("no clip"),
            FadeRefusal::UnknownClip
        );
    }
}
