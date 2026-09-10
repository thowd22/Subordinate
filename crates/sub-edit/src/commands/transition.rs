//! The crossfade commands: the only transition the MVP has (docs/PLAN.md
//! §5.3).
//!
//! A transition occupies no track time of its own. It sits in the item list
//! immediately before the clip it fades *into*, and blends across the cut by
//! reaching back into the outgoing clip by its in offset and forward into the
//! incoming clip by its out offset. Both commands here therefore address a cut
//! by naming the incoming clip: [`AddTransition`] puts a crossfade at the cut
//! before it, [`RemoveTransition`] takes it away again.
//!
//! # Clamping
//!
//! A crossfade cannot invent frames. Playing the outgoing clip past the cut
//! needs handle beyond its out point, and playing the incoming clip before the
//! cut needs handle before its in point, so the duration asked for is *clamped*
//! rather than refused, exactly as a trim is ([`crate::clip`]):
//!
//! | Half                            | Clamped to                                                     |
//! |---------------------------------|----------------------------------------------------------------|
//! | In offset, back into the outgoing clip | the outgoing clip's own duration, and the incoming clip's head handle |
//! | Out offset, into the incoming clip | the incoming clip's own duration, and the outgoing clip's tail handle |
//!
//! A clip whose media has never been probed has no known head or tail beyond
//! its source range, so only the neighbours' durations bound it; the head
//! handle is always known, being the distance from the start of the file.
//! A cut where both halves clamp to nothing is an error rather than a
//! zero-length transition: there is nothing there to blend.
//!
//! # Undo
//!
//! Both commands undo through [`RestoreTrackItems`], like every other edit
//! that rewrites a track's item list, so undoing an [`AddTransition`] that
//! replaced an existing crossfade puts the original offsets back exactly.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::{Clip, ClipId, Project, SequenceId, Track, TrackId, TrackItem, Transition};
use sub_time::{Rational, RationalTime, Rounding};

use super::track_for_clip_edit;
use crate::clip::{RestoreTrackItems, TrackItems};
use crate::codes;
use crate::{Command, Inverse};

/// Places a crossfade at the cut before a clip, clamped to the handles.
///
/// The duration asked for is split evenly across the cut — the odd frame of an
/// odd duration goes to the half before it — and each half is then clamped to
/// what the media and the neighbours allow. Applying the command where a
/// crossfade already sits replaces it, so dragging a transition's duration is
/// this one command rather than a remove and an add.
///
/// ```
/// use sub_edit::History;
/// use sub_edit::commands::AddTransition;
/// use sub_model::{
///     Clip, MediaItem, MediaPath, Project, Sequence, SequenceSettings, Track, TrackItem,
///     TrackKind,
/// };
/// use sub_time::{Rational, RationalTime, TimeRange};
///
/// let rate = Rational::FPS_24;
/// let frames = |value| RationalTime::new(value, rate);
/// let range = |start, len| TimeRange::new(frames(start), frames(len)).unwrap();
///
/// let mut project = Project::new("Doc cut");
/// let media = MediaItem::new(MediaPath::new("a.mp4").unwrap());
/// let media_id = media.id;
/// project.media.push(media);
///
/// // Two butt-joined clips, both cut from the middle of the file.
/// let outgoing = Clip::new("shot 1", media_id, range(48, 48));
/// let incoming = Clip::new("shot 2", media_id, range(240, 48));
/// let incoming_id = incoming.id;
/// let mut track = Track::new("V1", TrackKind::Video);
/// track.items.push(outgoing.into());
/// track.items.push(incoming.into());
/// let track_id = track.id;
/// let mut sequence = Sequence::new("Main", SequenceSettings::default());
/// sequence.tracks.push(track);
/// let sequence_id = sequence.id;
/// project.sequences.push(sequence);
///
/// let mut history = History::new();
/// history
///     .apply(
///         &mut project,
///         AddTransition::new(sequence_id, track_id, incoming_id, frames(12)),
///     )
///     .unwrap();
/// let transition = project.sequences[0].tracks[0].items[1]
///     .as_transition()
///     .copied()
///     .unwrap();
/// assert_eq!(transition.duration(), frames(12));
///
/// history.undo(&mut project).unwrap();
/// assert!(project.sequences[0].tracks[0].items[1].as_transition().is_none());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddTransition {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the cut.
    pub track: TrackId,
    /// The clip the crossfade fades into: the transition sits at its head.
    pub clip: ClipId,
    /// How long the blend should be, before clamping.
    pub duration: RationalTime,
}

impl AddTransition {
    /// A crossfade of `duration` at the cut before `clip`.
    #[must_use]
    pub const fn new(
        sequence: SequenceId,
        track: TrackId,
        clip: ClipId,
        duration: RationalTime,
    ) -> Self {
        Self {
            sequence,
            track,
            clip,
            duration,
        }
    }
}

impl Command for AddTransition {
    const KIND: &'static str = "transition.add";
    const DESCRIPTION: &'static str =
        "Place a crossfade at the cut before a clip, clamped to the available handles.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let fitted = fit_crossfade(project, self.sequence, self.track, self.clip, self.duration)?;
        let track = track_for_clip_edit(project, self.sequence, self.track)?;
        let cut = cut_at(track, self.clip)?;
        let mut items = track.items.clone();
        match cut.transition {
            Some(index) => items[index] = TrackItem::Transition(fitted),
            None => items.insert(cut.incoming, TrackItem::Transition(fitted)),
        }
        let previous = std::mem::replace(&mut track.items, items);
        Ok(restore(self.sequence, self.track, previous))
    }

    fn label(&self) -> String {
        "Add crossfade".to_owned()
    }
}

/// Takes the crossfade off the cut before a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveTransition {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the cut.
    pub track: TrackId,
    /// The clip the crossfade fades into.
    pub clip: ClipId,
}

impl RemoveTransition {
    /// The crossfade at the cut before `clip`.
    #[must_use]
    pub const fn new(sequence: SequenceId, track: TrackId, clip: ClipId) -> Self {
        Self {
            sequence,
            track,
            clip,
        }
    }
}

impl Command for RemoveTransition {
    const KIND: &'static str = "transition.remove";
    const DESCRIPTION: &'static str = "Remove the crossfade at the cut before a clip.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let track = track_for_clip_edit(project, self.sequence, self.track)?;
        let cut = cut_at(track, self.clip)?;
        let index = cut.transition.ok_or_else(|| {
            SubError::new(
                codes::TRANSITION_NOT_FOUND,
                "the cut before that clip carries no transition",
            )
            .with_detail("track_id", self.track)
            .with_detail("clip_id", self.clip)
        })?;
        let mut items = track.items.clone();
        items.remove(index);
        let previous = std::mem::replace(&mut track.items, items);
        Ok(restore(self.sequence, self.track, previous))
    }

    fn label(&self) -> String {
        "Remove crossfade".to_owned()
    }
}

/// The crossfade a request for `duration` at the cut before `clip` actually
/// becomes, once clamped to the handles either side of that cut.
///
/// The timeline uses this to draw the region a drag is about to commit,
/// so the preview and the command agree frame for frame.
///
/// # Errors
///
/// - `edit.sequence_not_found`, `edit.track_not_found` or `edit.clip_not_found`
///   when the cut cannot be found.
/// - `edit.invalid_transition` when `duration` is not positive, when the clip
///   has no clip butt-joined before it, or when neither side has any handle.
/// - `edit.invalid_time` when `duration` is not exactly representable at the
///   sequence timebase.
pub fn fit_crossfade(
    project: &Project,
    sequence: SequenceId,
    track: TrackId,
    clip: ClipId,
    duration: RationalTime,
) -> SubResult<Transition> {
    let rate = sequence_rate(project, sequence)?;
    let found = project
        .sequence(sequence)
        .and_then(|sequence| sequence.track(track))
        .ok_or_else(|| {
            SubError::new(codes::TRACK_NOT_FOUND, "no such track")
                .with_detail("sequence_id", sequence)
                .with_detail("track_id", track)
        })?;
    let cut = cut_at(found, clip)?;
    let outgoing = found.items[cut.outgoing]
        .as_clip()
        .ok_or_else(|| no_cut(track, clip))?;
    let incoming = found.items[cut.incoming]
        .as_clip()
        .ok_or_else(|| no_cut(track, clip))?;

    let requested = duration.checked_rescaled_to(rate).ok_or_else(|| {
        SubError::new(
            codes::INVALID_TIME,
            "a transition duration must be exact at the sequence timebase",
        )
        .with_detail("duration", duration.to_string())
    })?;
    if requested.is_negative() || requested.is_zero() {
        return Err(SubError::new(
            codes::INVALID_TRANSITION,
            "a transition lasts more than no time at all",
        )
        .with_detail("reason", "not_positive")
        .with_detail("duration", duration.to_string()));
    }

    // The odd frame of an odd duration goes to the half before the cut, so a
    // crossfade of one frame is one frame of the outgoing clip.
    let half = requested.value() / 2;
    let wanted_in = RationalTime::new(requested.value() - half, rate);
    let wanted_out = RationalTime::new(half, rate);

    let in_offset = wanted_in
        .min(floor_at(outgoing.duration(), rate))
        .min(floor_at(incoming.source_range.start(), rate));
    let out_offset = wanted_out
        .min(floor_at(incoming.duration(), rate))
        .min(tail_handle(project, outgoing, rate).unwrap_or(wanted_out));
    if in_offset.is_zero() && out_offset.is_zero() {
        return Err(SubError::new(
            codes::INVALID_TRANSITION,
            "neither side of the cut has any handle to blend across",
        )
        .with_detail("reason", "no_handle")
        .with_detail("track_id", track)
        .with_detail("clip_id", clip));
    }
    Ok(Transition::crossfade(in_offset, out_offset))
}

/// Where a cut sits in a track's item list.
#[derive(Debug, Clone, Copy)]
struct Cut {
    /// The clip before the cut.
    outgoing: usize,
    /// The clip after it, the one the commands name.
    incoming: usize,
    /// The transition already at the cut, when there is one.
    transition: Option<usize>,
}

/// The cut before `clip`, with the transition already on it.
///
/// # Errors
///
/// - `edit.clip_not_found` when the track does not hold `clip`.
/// - `edit.invalid_transition` when nothing is butt-joined before it: a gap,
///   or the head of the track.
fn cut_at(track: &Track, clip: ClipId) -> SubResult<Cut> {
    let incoming = track
        .items
        .iter()
        .position(|item| item.as_clip().is_some_and(|found| found.id == clip))
        .ok_or_else(|| {
            SubError::new(codes::CLIP_NOT_FOUND, "no such clip on the track")
                .with_detail("track_id", track.id)
                .with_detail("clip_id", clip)
        })?;
    let before = incoming
        .checked_sub(1)
        .ok_or_else(|| no_cut(track.id, clip))?;
    let (transition, outgoing) = if track.items[before].as_transition().is_some() {
        (
            Some(before),
            before
                .checked_sub(1)
                .ok_or_else(|| no_cut(track.id, clip))?,
        )
    } else {
        (None, before)
    };
    if track.items[outgoing].as_clip().is_none() {
        return Err(no_cut(track.id, clip));
    }
    Ok(Cut {
        outgoing,
        incoming,
        transition,
    })
}

/// The error for a clip with no clip butt-joined before it.
fn no_cut(track: TrackId, clip: ClipId) -> SubError {
    SubError::new(
        codes::INVALID_TRANSITION,
        "a transition needs a clip on both sides of the cut",
    )
    .with_detail("reason", "no_cut")
    .with_detail("track_id", track)
    .with_detail("clip_id", clip)
}

/// The inverse every command here returns: the track's items as they were.
fn restore(sequence: SequenceId, track: TrackId, items: Vec<TrackItem>) -> Inverse {
    Inverse::new(RestoreTrackItems {
        sequence,
        tracks: vec![TrackItems { track, items }],
    })
}

/// The sequence timebase.
fn sequence_rate(project: &Project, sequence: SequenceId) -> SubResult<Rational> {
    project
        .sequence(sequence)
        .map(|found| found.settings.frame_rate)
        .ok_or_else(|| {
            SubError::new(codes::SEQUENCE_NOT_FOUND, "no such sequence")
                .with_detail("sequence_id", sequence)
        })
}

/// How much media `clip` has past its out point, when its source has been
/// probed. `None` when it has not: an unprobed source bounds nothing.
fn tail_handle(project: &Project, clip: &Clip, rate: Rational) -> Option<RationalTime> {
    let duration = project.media_item(clip.media)?.info.as_ref()?.duration?;
    let handle = duration.checked_sub(clip.source_range.end_exclusive())?;
    if handle.is_negative() {
        return Some(RationalTime::zero(rate));
    }
    Some(floor_at(handle, rate))
}

/// `time` at the sequence timebase, never rounded up: an offset is clamped to
/// what the media really holds, so half a frame of handle is no frame at all.
/// A time that will not rescale at all bounds nothing and comes back as zero.
fn floor_at(time: RationalTime, rate: Rational) -> RationalTime {
    time.checked_rescaled_to_rounding(rate, Rounding::Floor)
        .filter(|floored| !floored.is_negative())
        .unwrap_or_else(|| RationalTime::zero(rate))
}

#[cfg(test)]
mod tests {
    use sub_model::media::StreamInfo;
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Gap, MediaItem, MediaPath, Sequence, TrackKind};
    use sub_time::TimeRange;

    use super::*;
    use crate::History;

    const RATE: Rational = Rational::FPS_24;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, RATE)
    }

    fn range(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(frames(start), frames(duration)).expect("a valid range")
    }

    /// A project whose V1 holds `a` at 0..48 cut from source 48..96 and `b` at
    /// 48..96 cut from source 240..288, both from a probed 480-frame file: 384
    /// frames of tail handle on `a`, 240 frames of head handle on `b`.
    fn scene(source_duration: Option<RationalTime>) -> (Project, SequenceId, TrackId, [ClipId; 2]) {
        let mut project = Project::new("crossfade");
        let mut item = MediaItem::new(MediaPath::new("media/take.mp4").expect("a valid path"));
        item.info = Some(StreamInfo {
            duration: source_duration,
            video: Vec::new(),
            audio: Vec::new(),
        });
        let media = item.id;
        project.media.push(item);

        let outgoing = Clip::new("a", media, range(48, 48));
        let incoming = Clip::new("b", media, range(240, 48));
        let ids = [outgoing.id, incoming.id];
        let mut track = Track::new("V1", TrackKind::Video);
        track.items.push(outgoing.into());
        track.items.push(incoming.into());
        let track_id = track.id;
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.tracks.push(track);
        let sequence_id = sequence.id;
        project.sequences.push(sequence);
        (project, sequence_id, track_id, ids)
    }

    fn transition_of(project: &Project) -> Option<Transition> {
        project.sequences[0].tracks[0]
            .items
            .iter()
            .find_map(|item| item.as_transition().copied())
    }

    #[test]
    fn a_crossfade_splits_evenly_across_the_cut() {
        let (mut project, sequence, track, [_, incoming]) = scene(Some(frames(480)));
        let mut history = History::new();
        history
            .apply(
                &mut project,
                AddTransition::new(sequence, track, incoming, frames(12)),
            )
            .expect("the crossfade applies");

        let transition = transition_of(&project).expect("a crossfade");
        assert_eq!(transition.in_offset(), frames(6));
        assert_eq!(transition.out_offset(), frames(6));
        assert_eq!(
            project.sequences[0].tracks[0].items.len(),
            3,
            "the transition sits between the two clips"
        );
        assert!(
            project.sequences[0].tracks[0].items[1]
                .as_transition()
                .is_some()
        );
    }

    #[test]
    fn an_odd_duration_puts_the_extra_frame_before_the_cut() {
        let (mut project, sequence, track, [_, incoming]) = scene(Some(frames(480)));
        let mut history = History::new();
        history
            .apply(
                &mut project,
                AddTransition::new(sequence, track, incoming, frames(7)),
            )
            .expect("the crossfade applies");
        let transition = transition_of(&project).expect("a crossfade");
        assert_eq!(transition.in_offset(), frames(4));
        assert_eq!(transition.out_offset(), frames(3));
        assert_eq!(transition.duration(), frames(7));
    }

    #[test]
    fn the_halves_are_clamped_to_the_neighbouring_clips() {
        let (mut project, sequence, track, [_, incoming]) = scene(Some(frames(480)));
        let mut history = History::new();
        // Each clip is 48 frames long, so a 200-frame request cannot have more
        // than 48 frames on either side of the cut.
        history
            .apply(
                &mut project,
                AddTransition::new(sequence, track, incoming, frames(200)),
            )
            .expect("the crossfade applies");
        let transition = transition_of(&project).expect("a crossfade");
        assert_eq!(transition.in_offset(), frames(48));
        assert_eq!(transition.out_offset(), frames(48));
    }

    #[test]
    fn the_out_half_is_clamped_to_the_outgoing_clips_tail_handle() {
        let (mut project, sequence, track, [_, incoming]) = scene(Some(frames(100)));
        // `a` is cut from 48..96 of a 100-frame file: four frames of tail.
        let fitted =
            fit_crossfade(&project, sequence, track, incoming, frames(40)).expect("a crossfade");
        assert_eq!(fitted.out_offset(), frames(4));
        assert_eq!(fitted.in_offset(), frames(20));
        let mut history = History::new();
        history
            .apply(
                &mut project,
                AddTransition::new(sequence, track, incoming, frames(40)),
            )
            .expect("the crossfade applies");
        assert_eq!(transition_of(&project), Some(fitted));
    }

    #[test]
    fn an_unprobed_source_is_bounded_only_by_the_clips() {
        let (project, sequence, track, [_, incoming]) = scene(None);
        let fitted =
            fit_crossfade(&project, sequence, track, incoming, frames(40)).expect("a crossfade");
        assert_eq!(fitted.in_offset(), frames(20));
        assert_eq!(fitted.out_offset(), frames(20));
    }

    #[test]
    fn a_cut_with_no_handle_at_all_is_refused() {
        let mut project = Project::new("crossfade");
        let mut item = MediaItem::new(MediaPath::new("media/take.mp4").expect("a valid path"));
        item.info = Some(StreamInfo {
            duration: Some(frames(48)),
            video: Vec::new(),
            audio: Vec::new(),
        });
        let media = item.id;
        project.media.push(item);
        // Both clips are the whole file: no tail on the first, no head on the
        // second.
        let outgoing = Clip::new("a", media, range(0, 48));
        let incoming = Clip::new("b", media, range(0, 48));
        let incoming_id = incoming.id;
        let mut track = Track::new("V1", TrackKind::Video);
        track.items.push(outgoing.into());
        track.items.push(incoming.into());
        let track_id = track.id;
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.tracks.push(track);
        let sequence_id = sequence.id;
        project.sequences.push(sequence);

        let err = fit_crossfade(&project, sequence_id, track_id, incoming_id, frames(12))
            .expect_err("nothing to blend");
        assert_eq!(err.code, codes::INVALID_TRANSITION);
        assert_eq!(
            err.details.get("reason"),
            Some(&serde_json::json!("no_handle"))
        );
    }

    #[test]
    fn a_clip_with_no_neighbour_has_no_cut() {
        let (mut project, sequence, track, [outgoing, incoming]) = scene(Some(frames(480)));
        let err = AddTransition::new(sequence, track, outgoing, frames(12))
            .apply(&mut project)
            .expect_err("the first clip has no cut before it");
        assert_eq!(err.code, codes::INVALID_TRANSITION);
        assert_eq!(
            err.details.get("reason"),
            Some(&serde_json::json!("no_cut"))
        );

        // Nor does a clip with a gap before it.
        project.sequences[0].tracks[0]
            .items
            .insert(1, TrackItem::Gap(Gap::new(frames(12))));
        let err = AddTransition::new(sequence, track, incoming, frames(12))
            .apply(&mut project)
            .expect_err("a gap is not a cut");
        assert_eq!(err.code, codes::INVALID_TRANSITION);
    }

    #[test]
    fn a_duration_of_nothing_is_refused() {
        let (mut project, sequence, track, [_, incoming]) = scene(Some(frames(480)));
        let err = AddTransition::new(sequence, track, incoming, frames(0))
            .apply(&mut project)
            .expect_err("a transition lasts some time");
        assert_eq!(err.code, codes::INVALID_TRANSITION);
        assert_eq!(
            err.details.get("reason"),
            Some(&serde_json::json!("not_positive"))
        );
    }

    #[test]
    fn adding_over_an_existing_crossfade_replaces_it_and_undoes_to_it() {
        let (mut project, sequence, track, [_, incoming]) = scene(Some(frames(480)));
        let mut history = History::new();
        history
            .apply(
                &mut project,
                AddTransition::new(sequence, track, incoming, frames(12)),
            )
            .expect("the first crossfade applies");
        history
            .apply(
                &mut project,
                AddTransition::new(sequence, track, incoming, frames(24)),
            )
            .expect("the second crossfade applies");

        assert_eq!(project.sequences[0].tracks[0].items.len(), 3);
        assert_eq!(
            transition_of(&project).expect("a crossfade").duration(),
            frames(24)
        );
        history.undo(&mut project).expect("undo").expect("a step");
        assert_eq!(
            transition_of(&project).expect("a crossfade").duration(),
            frames(12),
            "undo puts the original offsets back"
        );
    }

    #[test]
    fn removing_a_crossfade_undoes_to_it() {
        let (mut project, sequence, track, [_, incoming]) = scene(Some(frames(480)));
        let mut history = History::new();
        history
            .apply(
                &mut project,
                AddTransition::new(sequence, track, incoming, frames(12)),
            )
            .expect("the crossfade applies");
        history
            .apply(
                &mut project,
                RemoveTransition::new(sequence, track, incoming),
            )
            .expect("the crossfade goes");
        assert_eq!(transition_of(&project), None);
        assert_eq!(project.sequences[0].tracks[0].items.len(), 2);

        history.undo(&mut project).expect("undo").expect("a step");
        assert_eq!(
            transition_of(&project).expect("a crossfade").duration(),
            frames(12)
        );
    }

    #[test]
    fn removing_where_there_is_none_names_the_cut() {
        let (mut project, sequence, track, [_, incoming]) = scene(Some(frames(480)));
        let err = RemoveTransition::new(sequence, track, incoming)
            .apply(&mut project)
            .expect_err("no transition there");
        assert_eq!(err.code, codes::TRANSITION_NOT_FOUND);
    }

    #[test]
    fn a_locked_track_refuses_both_commands() {
        let (mut project, sequence, track, [_, incoming]) = scene(Some(frames(480)));
        project.sequences[0].tracks[0].locked = true;
        let err = AddTransition::new(sequence, track, incoming, frames(12))
            .apply(&mut project)
            .expect_err("locked");
        assert_eq!(err.code, codes::TRACK_LOCKED);
        let err = RemoveTransition::new(sequence, track, incoming)
            .apply(&mut project)
            .expect_err("locked");
        assert_eq!(err.code, codes::TRACK_LOCKED);
    }

    #[test]
    fn a_transition_takes_up_no_track_time() {
        let (mut project, sequence, track, [_, incoming]) = scene(Some(frames(480)));
        let before = project.sequences[0].tracks[0].duration(RATE);
        AddTransition::new(sequence, track, incoming, frames(12))
            .apply(&mut project)
            .expect("the crossfade applies");
        assert_eq!(project.sequences[0].tracks[0].duration(RATE), before);
    }
}
