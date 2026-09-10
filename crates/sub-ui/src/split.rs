//! Cutting clips: what `Ctrl+K` and the razor tool ask the Command API for.
//!
//! A cut is the basic verb of editing, and it is the same verb however it is
//! asked for. Both gestures reduce to the same thing here: an instant in
//! sequence time, the clips that instant falls strictly inside, and one
//! [`SplitClip`] per clip, applied inside a single history group so that one
//! undo puts the sequence back however many clips the cut went through.
//!
//! Nothing in this module mutates a project or knows about pixels. The panel
//! resolves a pointer to an exact [`RationalTime`] (through
//! [`crate::snapping`], so a cut lands on the frame the editor aimed at) and
//! plans the cut here; the application applies the plan with [`apply_split`].
//!
//! Two rules decide what a cut touches, and both are what an editor expects:
//!
//! - **With a selection, the selection decides.** `Ctrl+K` with clips
//!   selected cuts those clips and nothing else, even where other tracks have
//!   clips under the playhead.
//! - **With nothing selected, the playhead decides.** Every clip the instant
//!   falls inside, on every track that allows clip edits, is cut, so a
//!   through-edit across a stacked sequence is one keystroke.
//!
//! A clip is only cut where the instant falls *strictly* inside it: a cut on
//! a clip's own first frame would produce an empty head, and there is already
//! an edit there. Such a clip is skipped rather than refused, because a
//! through-edit that happens to line up with one clip's head is still a
//! perfectly good cut of the others.

use sub_core::{SubError, SubResult};
use sub_edit::clip::SplitClip;
use sub_model::{ClipId, Sequence};
use sub_time::{RationalTime, TimeRange};

use crate::codes;
use crate::selection::{ClipRef, Selection};
use crate::timeline_panel::clip_edits_allowed;

/// Why a cut cannot become an edit.
///
/// A cut that simply finds nothing to cut is not a refusal — it is
/// `Ok(None)`. These are the cases where the caller aimed at something that
/// cannot be cut at all, and the editor should say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SplitRefusal {
    /// The razor was used on a track that does not allow clip edits.
    LockedTrack,
    /// A named clip is no longer in the sequence.
    UnknownClip,
    /// The cut point is not inside the clip the razor named.
    NotInsideClip,
}

impl SplitRefusal {
    /// A stable identifier, for logs, errors and tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::LockedTrack => "locked_track",
            Self::UnknownClip => "unknown_clip",
            Self::NotInsideClip => "not_inside_clip",
        }
    }

    /// One sentence saying what is wrong, for the error and the tooltip.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::LockedTrack => "a locked track cannot be cut",
            Self::UnknownClip => "the clip to cut is no longer in the sequence",
            Self::NotInsideClip => "the cut point is not inside the clip",
        }
    }

    /// This refusal as a [`SubError`] carrying `ui.clip_split_refused`.
    #[must_use]
    pub fn to_error(self) -> SubError {
        SubError::new(codes::CLIP_SPLIT_REFUSED, self.message()).with_detail("reason", self.id())
    }
}

/// One clip the cut goes through, and what it leaves behind.
///
/// The head keeps the clip's identity and the tail gets [`SplitCut::tail`],
/// which is minted when the cut is planned rather than when it is applied, so
/// what a test asserts on and what the command creates are the same
/// identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitCut {
    /// The clip being cut, and the track it is on.
    pub from: ClipRef,
    /// The identity the tail will get.
    pub tail: ClipId,
    /// Where the cut falls, in sequence time.
    pub at: RationalTime,
    /// The span the head will occupy afterwards.
    pub head_range: TimeRange,
    /// The span the tail will occupy afterwards.
    pub tail_range: TimeRange,
}

/// One cut reduced to the commands it will commit.
///
/// [`SplitClip`] is a wire type rather than a value type, so the group is not
/// comparable; [`SplitGroup::cuts`] is what a test asserts on.
#[derive(Debug, Clone)]
pub struct SplitGroup {
    /// The label the one history entry gets.
    pub label: String,
    /// What each cut clip becomes, in track order.
    pub cuts: Vec<SplitCut>,
    /// The commands, in the order they must be applied.
    pub splits: Vec<SplitClip>,
}

impl SplitGroup {
    /// How many clips the cut goes through.
    #[must_use]
    pub fn len(&self) -> usize {
        self.splits.len()
    }

    /// Whether the cut goes through nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.splits.is_empty()
    }

    /// The cut as boxed commands, in the order they must be applied.
    ///
    /// This is what the app hands
    /// [`EditorSession::apply_group`](crate::session::EditorSession::apply_group),
    /// so a through-edit across several tracks is one entry in the undo stack.
    #[must_use]
    pub fn commands(&self) -> Vec<sub_edit::BoxedCommand> {
        self.splits
            .iter()
            .map(|command| Box::new(command.clone()) as sub_edit::BoxedCommand)
            .collect()
    }
}

/// Works out what cutting at `at` would do, given what is selected.
///
/// With a non-empty `selection` only the selected clips are considered; with
/// an empty one every clip of every editable track is. Clips the instant does
/// not fall strictly inside are skipped, and a cut that finds nothing to cut
/// yields `Ok(None)` so that a stray keystroke never pushes an undo step.
///
/// # Errors
///
/// Returns [`SplitRefusal::UnknownClip`] when the selection names a clip or a
/// track the sequence no longer has, which means an undo or another agent
/// took it away between frames.
pub fn plan_split(
    sequence: &Sequence,
    selection: &Selection,
    at: RationalTime,
) -> Result<Option<SplitGroup>, SplitRefusal> {
    let cuts = if selection.is_empty() {
        cuts_under(sequence, at)
    } else {
        cuts_of_selection(sequence, selection, at)?
    };
    Ok(group(sequence, cuts))
}

/// Works out what cutting the clip the razor was clicked on would do.
///
/// Unlike [`plan_split`] this refuses rather than skips: the razor names one
/// clip, so a cut that misses it means the caller aimed at the wrong clip or
/// the wrong instant and saying nothing would be worse than saying so.
///
/// # Errors
///
/// [`SplitRefusal::LockedTrack`] when the clip's track does not allow clip
/// edits, [`SplitRefusal::UnknownClip`] when the sequence has no such clip,
/// and [`SplitRefusal::NotInsideClip`] when `at` is outside the clip or on
/// its first frame.
pub fn plan_split_clip(
    sequence: &Sequence,
    item: ClipRef,
    at: RationalTime,
) -> Result<SplitGroup, SplitRefusal> {
    let rate = sequence.settings.frame_rate;
    let at = at.rescaled_to(rate);
    let track = sequence
        .tracks
        .iter()
        .find(|track| track.id == item.track)
        .ok_or(SplitRefusal::UnknownClip)?;
    if !clip_edits_allowed(track) {
        return Err(SplitRefusal::LockedTrack);
    }
    let range = track
        .placements(rate)
        .find_map(|(track_item, range)| {
            track_item
                .as_clip()
                .filter(|clip| clip.id == item.clip)
                .map(|_| range)
        })
        .ok_or(SplitRefusal::UnknownClip)?;
    let cut = cut_at(item, range, at).ok_or(SplitRefusal::NotInsideClip)?;
    group(sequence, vec![cut]).ok_or(SplitRefusal::NotInsideClip)
}

/// Every editable clip the instant falls strictly inside, in track order.
fn cuts_under(sequence: &Sequence, at: RationalTime) -> Vec<SplitCut> {
    let rate = sequence.settings.frame_rate;
    let at = at.rescaled_to(rate);
    let mut cuts = Vec::new();
    for track in &sequence.tracks {
        if !clip_edits_allowed(track) {
            continue;
        }
        for (track_item, range) in track.placements(rate) {
            if let Some(clip) = track_item.as_clip()
                && let Some(cut) = cut_at(ClipRef::new(track.id, clip.id), range, at)
            {
                cuts.push(cut);
            }
        }
    }
    cuts
}

/// The selected clips the instant falls strictly inside, in track order.
fn cuts_of_selection(
    sequence: &Sequence,
    selection: &Selection,
    at: RationalTime,
) -> Result<Vec<SplitCut>, SplitRefusal> {
    let rate = sequence.settings.frame_rate;
    let at = at.rescaled_to(rate);
    let mut cuts: Vec<(usize, SplitCut)> = Vec::new();
    for item in selection.items() {
        let (index, track) = sequence
            .tracks
            .iter()
            .enumerate()
            .find(|(_, track)| track.id == item.track)
            .ok_or(SplitRefusal::UnknownClip)?;
        if !clip_edits_allowed(track) {
            return Err(SplitRefusal::LockedTrack);
        }
        let range = track
            .placements(rate)
            .find_map(|(track_item, range)| {
                track_item
                    .as_clip()
                    .filter(|clip| clip.id == item.clip)
                    .map(|_| range)
            })
            .ok_or(SplitRefusal::UnknownClip)?;
        if let Some(cut) = cut_at(*item, range, at) {
            cuts.push((index, cut));
        }
    }
    // Selection order is the order the editor clicked in; a cut reads better
    // top to bottom, and the commands are independent of each other either
    // way because no clip moves.
    cuts.sort_by_key(|(index, cut)| (*index, cut.head_range.start()));
    Ok(cuts.into_iter().map(|(_, cut)| cut).collect())
}

/// The cut a clip spanning `range` takes at `at`, when it takes one.
///
/// `None` where the instant is outside the clip or on its first frame: there
/// is nothing to cut, and a cut that left an empty head would not be one.
fn cut_at(from: ClipRef, range: TimeRange, at: RationalTime) -> Option<SplitCut> {
    if !range.contains(at) || at == range.start() {
        return None;
    }
    let head = TimeRange::from_start_end(range.start(), at)?;
    let tail = TimeRange::from_start_end(at, range.end_exclusive())?;
    Some(SplitCut {
        from,
        // `ClipId::new` mints a fresh identifier.
        tail: ClipId::new(),
        at,
        head_range: head,
        tail_range: tail,
    })
}

/// Turns the cuts into a group, or `None` when there are none.
fn group(sequence: &Sequence, cuts: Vec<SplitCut>) -> Option<SplitGroup> {
    if cuts.is_empty() {
        return None;
    }
    let splits = cuts
        .iter()
        .map(|cut| SplitClip {
            sequence: sequence.id,
            track: cut.from.track,
            clip: cut.from.clip,
            at: cut.at,
            tail_id: Some(cut.tail),
        })
        .collect();
    Some(SplitGroup {
        label: split_label(cuts.len()),
        cuts,
        splits,
    })
}

/// The history label for a cut that went through `count` clips.
fn split_label(count: usize) -> String {
    if count == 1 {
        "Split clip".to_owned()
    } else {
        format!("Split {count} clips")
    }
}

/// Applies a planned cut as one undoable step.
///
/// The whole group goes into a single [`History`](sub_edit::History) entry, so
/// undoing a through-edit rejoins every clip it went through at once. A
/// command that fails takes the group with it: the history rolls back what it
/// already applied, leaving the project as it was before the cut.
///
/// # Errors
///
/// Whatever a [`SplitClip`] returns, and `edit.group_open` when a group is
/// already open.
pub fn apply_split(
    history: &mut sub_edit::History,
    project: &mut sub_model::Project,
    group: &SplitGroup,
) -> SubResult<()> {
    history.begin_group(group.label.clone())?;
    for command in &group.splits {
        history.apply(project, command.clone())?;
    }
    history.commit_group()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sub_model::MediaPath;
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Clip, Gap, MediaItem, Track, TrackItem, TrackKind};
    use sub_time::Rational;

    use super::*;

    const RATE: Rational = Rational::FPS_24;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, RATE)
    }

    fn span(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(frames(start), frames(duration)).expect("a valid range")
    }

    /// V1: a clip at 0..48 and another at 96..144. V2: one at 0..144.
    /// V3: locked, holding a clip at 0..48.
    fn scene() -> Sequence {
        let media = MediaItem::new(MediaPath::new("media/take.mp4").expect("a valid path")).id;
        let mut sequence = Sequence::new("edit", SequenceSettings::default());

        let mut top = Track::new("V1", TrackKind::Video);
        top.items
            .push(TrackItem::Clip(Clip::new("head", media, span(0, 48))));
        top.items.push(TrackItem::Gap(Gap::new(frames(48))));
        top.items
            .push(TrackItem::Clip(Clip::new("tail", media, span(0, 48))));
        sequence.tracks.push(top);

        let mut middle = Track::new("V2", TrackKind::Video);
        middle
            .items
            .push(TrackItem::Clip(Clip::new("under", media, span(0, 144))));
        sequence.tracks.push(middle);

        let mut locked = Track::new("V3", TrackKind::Video);
        locked.locked = true;
        locked
            .items
            .push(TrackItem::Clip(Clip::new("bolted", media, span(0, 48))));
        sequence.tracks.push(locked);

        sequence
    }

    /// The clip on `track` at `index`, as a reference.
    fn clip_ref(sequence: &Sequence, track: usize, index: usize) -> ClipRef {
        let track = &sequence.tracks[track];
        let clip = track
            .items
            .iter()
            .filter_map(TrackItem::as_clip)
            .nth(index)
            .expect("the scene has that clip");
        ClipRef::new(track.id, clip.id)
    }

    #[test]
    fn with_nothing_selected_a_cut_goes_through_every_editable_track() {
        let sequence = scene();
        let group = plan_split(&sequence, &Selection::new(), frames(24))
            .expect("a legal cut")
            .expect("something to cut");
        assert_eq!(group.len(), 2, "V1's head and V2's clip, but not locked V3");
        assert_eq!(group.label, "Split 2 clips");
        assert_eq!(
            group.cuts[0].head_range,
            span(0, 24),
            "the head keeps the clip's place"
        );
        assert_eq!(group.cuts[0].tail_range, span(24, 24));
        assert_eq!(group.cuts[1].tail_range, span(24, 120));
    }

    #[test]
    fn a_selection_decides_what_a_cut_touches() {
        let sequence = scene();
        let mut selection = Selection::new();
        selection.select_only(clip_ref(&sequence, 0, 0));
        let group = plan_split(&sequence, &selection, frames(24))
            .expect("a legal cut")
            .expect("something to cut");
        assert_eq!(group.len(), 1);
        assert_eq!(group.label, "Split clip");
        assert_eq!(group.cuts[0].from, clip_ref(&sequence, 0, 0));
    }

    #[test]
    fn a_cut_no_clip_contains_is_not_an_edit() {
        let sequence = scene();
        let mut selection = Selection::new();
        selection.select_only(clip_ref(&sequence, 0, 0));
        // Frame 60 is in the gap between V1's two clips.
        assert!(
            plan_split(&sequence, &selection, frames(60))
                .expect("a legal cut")
                .is_none()
        );
    }

    #[test]
    fn a_cut_on_a_clips_first_frame_is_skipped_and_the_others_are_still_made() {
        let sequence = scene();
        // Frame 96 is V1's second clip's own head, and strictly inside V2's.
        let group = plan_split(&sequence, &Selection::new(), frames(96))
            .expect("a legal cut")
            .expect("something to cut");
        assert_eq!(group.len(), 1);
        assert_eq!(group.cuts[0].from, clip_ref(&sequence, 1, 0));
    }

    #[test]
    fn every_cut_mints_its_own_tail_identity() {
        let sequence = scene();
        let group = plan_split(&sequence, &Selection::new(), frames(24))
            .expect("a legal cut")
            .expect("something to cut");
        assert_ne!(group.cuts[0].tail, group.cuts[1].tail);
        for (cut, command) in group.cuts.iter().zip(&group.splits) {
            assert_eq!(command.tail_id, Some(cut.tail), "the plan is the command");
            assert_eq!(command.at, cut.at);
            assert_eq!(command.clip, cut.from.clip);
        }
    }

    #[test]
    fn the_razor_refuses_a_locked_track() {
        let sequence = scene();
        let refusal = plan_split_clip(&sequence, clip_ref(&sequence, 2, 0), frames(24))
            .expect_err("a locked track cannot be cut");
        assert_eq!(refusal, SplitRefusal::LockedTrack);
        assert_eq!(refusal.to_error().code, codes::CLIP_SPLIT_REFUSED);
    }

    #[test]
    fn the_razor_refuses_a_cut_outside_the_clip_it_named() {
        let sequence = scene();
        let refusal = plan_split_clip(&sequence, clip_ref(&sequence, 0, 0), frames(60))
            .expect_err("frame 60 is past the end of that clip");
        assert_eq!(refusal, SplitRefusal::NotInsideClip);
    }

    #[test]
    fn a_selection_the_sequence_no_longer_has_is_a_refusal() {
        let sequence = scene();
        let mut selection = Selection::new();
        selection.select_only(ClipRef::new(sequence.tracks[0].id, ClipId::new()));
        let refusal = plan_split(&sequence, &selection, frames(24))
            .expect_err("the clip is not on that track");
        assert_eq!(refusal, SplitRefusal::UnknownClip);
    }

    #[test]
    fn every_refusal_has_a_distinct_stable_id() {
        let all = [
            SplitRefusal::LockedTrack,
            SplitRefusal::UnknownClip,
            SplitRefusal::NotInsideClip,
        ];
        let mut ids: Vec<&str> = all.iter().map(|refusal| refusal.id()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "ids are distinct");
    }
}
