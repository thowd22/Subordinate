//! What is selected on the timeline, and what dragging a selection means.
//!
//! Selection is view state: it is what the editor is pointing at, not part of
//! the edit, so it lives in the panel and never in the project. Moving what is
//! selected is the opposite — every clip that shifts does so through a
//! [`MoveClip`] command, and a drag that shifts five clips at once still has
//! to be one step in the undo stack. That is what [`MoveGroup`] is: the
//! ordered commands a drag produced plus the label the history entry gets, for
//! the caller to apply inside `History::begin_group`/`commit_group`.
//!
//! Nothing here touches pixels. A drag arrives already reduced to two exact
//! numbers — how far in sequence time and how many lanes — so the plan is
//! computed in [`RationalTime`] and integer track indexes, and a refused drag
//! is refused for a modelled reason rather than a pixel one.

use sub_core::{SubError, SubResult};
use sub_edit::clip::MoveClip;
use sub_model::{ClipId, Sequence, TrackId};
use sub_time::{RationalTime, TimeRange};

use crate::codes;
use crate::timeline_panel::clip_edits_allowed;

/// One selected clip, and the track it is on.
///
/// A [`ClipId`] is unique on its own; the track comes along because every clip
/// command names the track the clip is on, and finding it again would mean
/// walking the sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClipRef {
    /// The track the clip sits on.
    pub track: TrackId,
    /// The clip itself.
    pub clip: ClipId,
}

impl ClipRef {
    /// A reference to `clip` on `track`.
    #[must_use]
    pub const fn new(track: TrackId, clip: ClipId) -> Self {
        Self { track, clip }
    }
}

/// The clips the editor has selected, in the order they were selected.
///
/// Insertion order is kept because it is what an editor sees: the clip clicked
/// first is the anchor of the gesture. Membership is a linear scan, which is
/// the right shape here — a selection is a handful of clips, not a sequence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    items: Vec<ClipRef>,
}

impl Selection {
    /// An empty selection.
    #[must_use]
    pub const fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Whether `item` is selected.
    #[must_use]
    pub fn contains(&self, item: ClipRef) -> bool {
        self.items.contains(&item)
    }

    /// Whether `clip` is selected, whatever track it is on.
    #[must_use]
    pub fn contains_clip(&self, clip: ClipId) -> bool {
        self.items.iter().any(|item| item.clip == clip)
    }

    /// How many clips are selected.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether nothing is selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The selected clips, in selection order.
    #[must_use]
    pub fn items(&self) -> &[ClipRef] {
        &self.items
    }

    /// Deselects everything, and says whether that changed anything.
    pub fn clear(&mut self) -> bool {
        let changed = !self.items.is_empty();
        self.items.clear();
        changed
    }

    /// Selects `item` and nothing else: a plain click.
    ///
    /// Returns whether the selection changed, so a caller can skip a repaint
    /// when clicking the already-only-selected clip.
    pub fn select_only(&mut self, item: ClipRef) -> bool {
        if self.items.len() == 1 && self.items[0] == item {
            return false;
        }
        self.items.clear();
        self.items.push(item);
        true
    }

    /// Adds `item` if it is absent and removes it if it is present: a
    /// shift-click.
    ///
    /// Always a change, so it always returns `true`; the return type matches
    /// the other mutators so a caller can `|=` them together.
    pub fn toggle(&mut self, item: ClipRef) -> bool {
        if let Some(index) = self.items.iter().position(|held| *held == item) {
            self.items.remove(index);
        } else {
            self.items.push(item);
        }
        true
    }

    /// Adds `item` if it is not selected already.
    pub fn insert(&mut self, item: ClipRef) -> bool {
        if self.contains(item) {
            return false;
        }
        self.items.push(item);
        true
    }

    /// Replaces the selection with `items`, keeping their order.
    ///
    /// This is what a marquee commits: it computed the whole answer, so it
    /// replaces rather than accumulates. A shift-marquee adds instead, through
    /// [`Selection::insert`].
    pub fn set<I: IntoIterator<Item = ClipRef>>(&mut self, items: I) -> bool {
        let replacement: Vec<ClipRef> = items.into_iter().collect();
        if replacement == self.items {
            return false;
        }
        self.items = replacement;
        true
    }

    /// Drops any selected clip that is no longer in `sequence`.
    ///
    /// A selection outlives the edits made around it, so a clip that was
    /// removed, rippled away or merged has to leave the selection with it
    /// rather than reappear as a phantom in the next drag.
    pub fn retain_existing(&mut self, sequence: &Sequence) -> bool {
        let before = self.items.len();
        self.items.retain(|item| {
            sequence.tracks.iter().any(|track| {
                track.id == item.track
                    && track
                        .items
                        .iter()
                        .filter_map(sub_model::TrackItem::as_clip)
                        .any(|clip| clip.id == item.clip)
            })
        });
        before != self.items.len()
    }
}

/// Why a drag cannot become an edit.
///
/// The panel paints a refused drag differently rather than silently doing
/// nothing, so the reason is modelled and not just a boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveRefusal {
    /// A selected clip would land before sequence time zero.
    BeforeStart,
    /// The clip is on, or would land on, a locked track.
    LockedTrack,
    /// The drag would carry a clip off the top or the bottom of the sequence.
    NoSuchTrack,
    /// A selected clip is no longer in the sequence.
    UnknownClip,
    /// The arithmetic of the move overflowed, which no real edit does.
    OutOfRange,
}

impl MoveRefusal {
    /// A stable identifier, for logging and for tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::BeforeStart => "before_start",
            Self::LockedTrack => "locked_track",
            Self::NoSuchTrack => "no_such_track",
            Self::UnknownClip => "unknown_clip",
            Self::OutOfRange => "out_of_range",
        }
    }

    /// One sentence saying what is wrong, for the error and the tooltip.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::BeforeStart => "a clip would land before the start of the sequence",
            Self::LockedTrack => "a locked track cannot give up or take a clip",
            Self::NoSuchTrack => "the drag leaves the sequence's tracks",
            Self::UnknownClip => "a selected clip is no longer in the sequence",
            Self::OutOfRange => "the move is too far to represent",
        }
    }

    /// This refusal as a [`SubError`] carrying `ui.clip_move_refused`.
    #[must_use]
    pub fn to_error(self) -> SubError {
        SubError::new(codes::CLIP_MOVE_REFUSED, self.message()).with_detail("reason", self.id())
    }
}

/// Where one dragged clip would end up.
///
/// The panel paints these as ghost rectangles; the same values become the
/// [`MoveClip`] commands, so what is previewed and what is committed cannot
/// drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewedMove {
    /// The clip being moved, and the track it is on now.
    pub from: ClipRef,
    /// The track index it would land on.
    pub to_track_index: usize,
    /// The track it would land on.
    pub to_track: TrackId,
    /// The span it would occupy afterwards.
    pub range: TimeRange,
}

/// One drag reduced to the commands it will commit.
///
/// The commands are ordered so that applying them in order never has a clip
/// overwrite one that has not moved out of the way yet: a drag to the right
/// moves the right-most clip first, a drag to the left the left-most.
///
/// [`MoveClip`] is a wire type rather than a value type, so the group is not
/// comparable; [`MoveGroup::previews`] is what a test asserts on.
#[derive(Debug, Clone)]
pub struct MoveGroup {
    /// The label the one history entry gets.
    pub label: String,
    /// Where each dragged clip ends up, in the order it is painted.
    pub previews: Vec<PreviewedMove>,
    /// The commands, in the order they must be applied.
    pub moves: Vec<MoveClip>,
}

impl MoveGroup {
    /// How many clips the drag moves.
    #[must_use]
    pub fn len(&self) -> usize {
        self.moves.len()
    }

    /// Whether the drag moves nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// The drag as boxed commands, in the order they must be applied.
    ///
    /// This is what the app hands
    /// [`EditorSession::apply_group`](crate::session::EditorSession::apply_group),
    /// so the whole drag becomes one entry in the engine's undo stack.
    #[must_use]
    pub fn commands(&self) -> Vec<sub_edit::BoxedCommand> {
        self.moves
            .iter()
            .map(|command| Box::new(command.clone()) as sub_edit::BoxedCommand)
            .collect()
    }
}

/// Works out what dragging `selection` by `delta` and `track_delta` lanes
/// would do.
///
/// `delta` is a time offset at the sequence timebase and `track_delta` a
/// number of lanes, positive downwards in [`Sequence::tracks`] order. A zero
/// drag is not an edit and yields `Ok(None)`, so a click that wobbled a pixel
/// does not push an undo step.
///
/// # Errors
///
/// Returns [`MoveRefusal`] when the drag cannot be made: a clip would land
/// before the head of the sequence, either end of the move touches a locked
/// track, or the drag leaves the sequence's tracks altogether.
pub fn plan_move(
    sequence: &Sequence,
    selection: &Selection,
    delta: RationalTime,
    track_delta: isize,
) -> Result<Option<MoveGroup>, MoveRefusal> {
    if selection.is_empty() {
        return Ok(None);
    }
    if delta.is_zero() && track_delta == 0 {
        return Ok(None);
    }
    let rate = sequence.settings.frame_rate;
    let delta = delta.rescaled_to(rate);
    let mut previews = Vec::with_capacity(selection.len());
    for item in selection.items() {
        let (index, track) = sequence
            .tracks
            .iter()
            .enumerate()
            .find(|(_, track)| track.id == item.track)
            .ok_or(MoveRefusal::UnknownClip)?;
        if !clip_edits_allowed(track) {
            return Err(MoveRefusal::LockedTrack);
        }
        let range = track
            .placements(rate)
            .find_map(|(track_item, range)| {
                track_item
                    .as_clip()
                    .filter(|clip| clip.id == item.clip)
                    .map(|_| range)
            })
            .ok_or(MoveRefusal::UnknownClip)?;
        let target_index = index
            .checked_add_signed(track_delta)
            .ok_or(MoveRefusal::NoSuchTrack)?;
        let target = sequence
            .tracks
            .get(target_index)
            .ok_or(MoveRefusal::NoSuchTrack)?;
        if !clip_edits_allowed(target) {
            return Err(MoveRefusal::LockedTrack);
        }
        if target.kind != track.kind {
            // A picture clip has nothing to be on an audio track, and the
            // reverse; the drag stops at the boundary rather than silently
            // converting the clip.
            return Err(MoveRefusal::NoSuchTrack);
        }
        let start = range
            .start()
            .checked_add(delta)
            .ok_or(MoveRefusal::OutOfRange)?;
        if start.is_negative() {
            return Err(MoveRefusal::BeforeStart);
        }
        let range = TimeRange::new(start, range.duration()).ok_or(MoveRefusal::OutOfRange)?;
        previews.push(PreviewedMove {
            from: *item,
            to_track_index: target_index,
            to_track: target.id,
            range,
        });
    }

    let mut ordered: Vec<PreviewedMove> = previews.clone();
    // Later-starting clips move first when the drag goes right, so a clip
    // never overwrites one of its own selection that is still where it was.
    if delta.is_negative() {
        ordered.sort_by_key(|preview| preview.range.start());
    } else {
        ordered.sort_by_key(|preview| core::cmp::Reverse(preview.range.start()));
    }
    let moves = ordered
        .iter()
        .map(|preview| MoveClip {
            sequence: sequence.id,
            track: preview.from.track,
            clip: preview.from.clip,
            start: preview.range.start(),
            to_track: (preview.to_track != preview.from.track).then_some(preview.to_track),
        })
        .collect();
    Ok(Some(MoveGroup {
        label: move_label(previews.len()),
        previews,
        moves,
    }))
}

/// The history label for a drag that moved `count` clips.
fn move_label(count: usize) -> String {
    if count == 1 {
        "Move clip".to_owned()
    } else {
        format!("Move {count} clips")
    }
}

/// Applies a planned drag as one undoable step.
///
/// The whole group goes into a single [`History`](sub_edit::History) entry, so
/// undo puts every dragged clip back at once. A command that fails takes the
/// group with it: the history rolls back what it already applied, leaving the
/// project as it was before the drag.
///
/// # Errors
///
/// Whatever a [`MoveClip`] returns, and `edit.group_open` when a group is
/// already open.
pub fn apply_move(
    history: &mut sub_edit::History,
    project: &mut sub_model::Project,
    group: &MoveGroup,
) -> SubResult<()> {
    history.begin_group(group.label.clone())?;
    for command in &group.moves {
        history.apply(project, command.clone())?;
    }
    history.commit_group()?;
    Ok(())
}

/// The clips of `sequence` whose rectangles a marquee covers.
///
/// `tracks` is the half-open range of lane indexes the rubber band touches and
/// `span` the time it covers; a clip is caught when it is on one of those
/// tracks and overlaps that span. Locked tracks are skipped: their clips
/// cannot be edited, so selecting them would only offer a gesture that is
/// about to be refused.
#[must_use]
pub fn clips_in_marquee(
    sequence: &Sequence,
    tracks: core::ops::Range<usize>,
    span: TimeRange,
) -> Vec<ClipRef> {
    let rate = sequence.settings.frame_rate;
    let mut caught = Vec::new();
    if span.is_empty() {
        return caught;
    }
    for index in tracks {
        let Some(track) = sequence.tracks.get(index) else {
            continue;
        };
        if !clip_edits_allowed(track) {
            continue;
        }
        for (item, range) in track.placements(rate) {
            if let Some(clip) = item.as_clip()
                && range.overlaps(span)
            {
                caught.push(ClipRef::new(track.id, clip.id));
            }
        }
    }
    caught
}

/// Turns a refusal into the error the Command API would have raised.
///
/// The panel refuses a drag before it builds a command, so nothing reaches the
/// history; this is for the callers that want to log or report the refusal in
/// the same shape as every other failure.
///
/// # Errors
///
/// Always: it is a refusal.
pub fn refuse<T>(refusal: MoveRefusal) -> SubResult<T> {
    Err(refusal.to_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Clip, Gap, MediaItem, MediaPath, Project, Track, TrackItem, TrackKind};
    use sub_time::Rational;

    const RATE: Rational = Rational::FPS_24;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, RATE)
    }

    fn range(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(frames(start), frames(duration)).expect("a valid range")
    }

    /// A sequence with two video tracks and one audio track:
    /// V1 holds `head` at 0..48 and `tail` at 96..144, V2 is empty.
    fn scene() -> (Project, Sequence) {
        let mut project = Project::new("selection");
        let item = MediaItem::new(MediaPath::new("media/take.mp4").expect("a valid path"));
        let media = item.id;
        project.media.push(item);

        let mut sequence = Sequence::new("edit", SequenceSettings::default());
        let mut lower = Track::new("V1", TrackKind::Video);
        lower
            .items
            .push(TrackItem::Clip(Clip::new("head", media, range(0, 48))));
        lower.items.push(TrackItem::Gap(Gap::new(frames(48))));
        lower
            .items
            .push(TrackItem::Clip(Clip::new("tail", media, range(0, 48))));
        sequence.tracks.push(lower);
        sequence.tracks.push(Track::new("V2", TrackKind::Video));
        sequence.tracks.push(Track::new("A1", TrackKind::Audio));
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

    fn selection_of(items: &[ClipRef]) -> Selection {
        let mut selection = Selection::new();
        selection.set(items.iter().copied());
        selection
    }

    #[test]
    fn a_click_selects_one_clip_and_shift_click_toggles() {
        let (_, sequence) = scene();
        let head = clip_ref(&sequence, 0, 0);
        let tail = clip_ref(&sequence, 0, 1);
        let mut selection = Selection::new();

        assert!(selection.select_only(head));
        assert_eq!(selection.items(), [head]);
        assert!(
            !selection.select_only(head),
            "clicking the only selected clip changes nothing"
        );

        assert!(selection.toggle(tail));
        assert_eq!(selection.items(), [head, tail], "shift-click extends");
        assert!(selection.toggle(head));
        assert_eq!(selection.items(), [tail], "and shift-click again removes");

        assert!(selection.select_only(head), "a plain click replaces");
        assert_eq!(selection.items(), [head]);
        assert!(selection.clear());
        assert!(selection.is_empty());
        assert!(!selection.clear(), "clearing nothing is not a change");
    }

    #[test]
    fn a_marquee_catches_the_clips_it_covers() {
        let (_, sequence) = scene();
        let head = clip_ref(&sequence, 0, 0);
        let tail = clip_ref(&sequence, 0, 1);

        let caught = clips_in_marquee(&sequence, 0..3, range(0, 200));
        assert_eq!(caught, vec![head, tail], "the band covers both clips");

        let caught = clips_in_marquee(&sequence, 0..3, range(100, 8));
        assert_eq!(caught, vec![tail], "and here only the second one");

        let caught = clips_in_marquee(&sequence, 1..3, range(0, 200));
        assert!(caught.is_empty(), "the other tracks hold no clips");

        let caught = clips_in_marquee(&sequence, 0..3, TimeRange::empty_at(frames(10)));
        assert!(caught.is_empty(), "an empty band catches nothing");
    }

    #[test]
    fn a_marquee_skips_locked_tracks() {
        let (_, mut sequence) = scene();
        sequence.tracks[0].locked = true;
        assert!(
            clips_in_marquee(&sequence, 0..3, range(0, 200)).is_empty(),
            "a locked track's clips cannot be edited, so they are not caught"
        );
    }

    #[test]
    fn a_drag_becomes_one_move_command_per_selected_clip() {
        let (_, sequence) = scene();
        let head = clip_ref(&sequence, 0, 0);
        let tail = clip_ref(&sequence, 0, 1);
        let selection = selection_of(&[head, tail]);

        let group = plan_move(&sequence, &selection, frames(12), 0)
            .expect("a legal drag")
            .expect("a drag that moved");
        assert_eq!(group.label, "Move 2 clips");
        assert_eq!(group.len(), 2);
        assert!(
            group.moves.iter().all(|command| command.to_track.is_none()),
            "a drag within one track names no destination track"
        );
        assert_eq!(
            group.moves[0].start,
            frames(108),
            "dragging right moves the right-most clip first"
        );
        assert_eq!(group.moves[1].start, frames(12));

        let tail_only = selection_of(&[tail]);
        let group = plan_move(&sequence, &tail_only, frames(-12), 0)
            .expect("a legal drag")
            .expect("a drag that moved");
        assert_eq!(
            group.moves[0].start,
            frames(84),
            "dragging left moves the left-most clip first"
        );
    }

    #[test]
    fn a_drag_that_moves_nothing_is_not_an_edit() {
        let (_, sequence) = scene();
        let selection = selection_of(&[clip_ref(&sequence, 0, 0)]);
        assert!(
            plan_move(&sequence, &selection, frames(0), 0)
                .expect("no refusal")
                .is_none()
        );
        assert!(
            plan_move(&sequence, &Selection::new(), frames(12), 0)
                .expect("no refusal")
                .is_none(),
            "and neither is dragging an empty selection"
        );
    }

    #[test]
    fn a_drag_onto_another_track_names_the_destination() {
        let (_, sequence) = scene();
        let selection = selection_of(&[clip_ref(&sequence, 0, 0)]);
        let group = plan_move(&sequence, &selection, frames(0), 1)
            .expect("a legal drag")
            .expect("a drag that moved");
        assert_eq!(group.label, "Move clip");
        assert_eq!(group.moves[0].to_track, Some(sequence.tracks[1].id));
        assert_eq!(group.moves[0].start, frames(0));
        assert_eq!(group.previews[0].to_track_index, 1);
    }

    #[test]
    fn a_drag_before_the_head_of_the_sequence_is_refused() {
        let (_, sequence) = scene();
        let selection = selection_of(&[clip_ref(&sequence, 0, 0)]);
        assert_eq!(
            plan_move(&sequence, &selection, frames(-1), 0).unwrap_err(),
            MoveRefusal::BeforeStart
        );
        let error = MoveRefusal::BeforeStart.to_error();
        assert_eq!(error.code, codes::CLIP_MOVE_REFUSED);
    }

    #[test]
    fn a_drag_touching_a_locked_track_is_refused() {
        let (_, mut sequence) = scene();
        let selection = selection_of(&[clip_ref(&sequence, 0, 0)]);

        sequence.tracks[1].locked = true;
        assert_eq!(
            plan_move(&sequence, &selection, frames(0), 1).unwrap_err(),
            MoveRefusal::LockedTrack,
            "a locked track cannot take a clip"
        );

        sequence.tracks[1].locked = false;
        sequence.tracks[0].locked = true;
        assert_eq!(
            plan_move(&sequence, &selection, frames(12), 0).unwrap_err(),
            MoveRefusal::LockedTrack,
            "nor can one give a clip up"
        );
    }

    #[test]
    fn a_drag_off_the_end_of_the_tracks_is_refused() {
        let (_, sequence) = scene();
        let selection = selection_of(&[clip_ref(&sequence, 0, 0)]);
        assert_eq!(
            plan_move(&sequence, &selection, frames(0), -1).unwrap_err(),
            MoveRefusal::NoSuchTrack
        );
        assert_eq!(
            plan_move(&sequence, &selection, frames(0), 9).unwrap_err(),
            MoveRefusal::NoSuchTrack
        );
        assert_eq!(
            plan_move(&sequence, &selection, frames(0), 2).unwrap_err(),
            MoveRefusal::NoSuchTrack,
            "and a picture clip does not belong on an audio track"
        );
    }

    #[test]
    fn a_planned_drag_applies_as_one_undo_step() {
        let (mut project, sequence) = scene();
        let selection = selection_of(&[clip_ref(&sequence, 0, 0), clip_ref(&sequence, 0, 1)]);
        let group = plan_move(&sequence, &selection, frames(24), 0)
            .expect("a legal drag")
            .expect("a drag that moved");

        let mut history = sub_edit::History::new();
        apply_move(&mut history, &mut project, &group).expect("the group applies");
        assert_eq!(history.undo_len(), 1, "a drag is one step, not two");

        let moved = &project.sequences[0].tracks[0];
        let starts: Vec<i64> = moved
            .placements(RATE)
            .filter(|(item, _)| item.as_clip().is_some())
            .map(|(_, range)| range.start().value())
            .collect();
        assert_eq!(starts, vec![24, 120]);

        history.undo(&mut project).expect("undo works");
        let restored = &project.sequences[0].tracks[0];
        let starts: Vec<i64> = restored
            .placements(RATE)
            .filter(|(item, _)| item.as_clip().is_some())
            .map(|(_, range)| range.start().value())
            .collect();
        assert_eq!(starts, vec![0, 96], "one undo puts both clips back");
    }

    #[test]
    fn a_selection_drops_clips_that_left_the_sequence() {
        let (_, mut sequence) = scene();
        let head = clip_ref(&sequence, 0, 0);
        let tail = clip_ref(&sequence, 0, 1);
        let mut selection = selection_of(&[head, tail]);

        sequence.tracks[0].items.truncate(1);
        assert!(selection.retain_existing(&sequence));
        assert_eq!(selection.items(), [head]);
        assert!(
            !selection.retain_existing(&sequence),
            "and a selection that is already sound is left alone"
        );
    }

    #[test]
    fn every_refusal_has_a_stable_id() {
        for refusal in [
            MoveRefusal::BeforeStart,
            MoveRefusal::LockedTrack,
            MoveRefusal::NoSuchTrack,
            MoveRefusal::UnknownClip,
            MoveRefusal::OutOfRange,
        ] {
            assert!(!refusal.id().is_empty());
            assert!(!refusal.message().is_empty());
            assert_eq!(
                refusal.to_error().code,
                codes::CLIP_MOVE_REFUSED,
                "every refusal reports the one code"
            );
        }
        assert!(refuse::<()>(MoveRefusal::UnknownClip).is_err());
    }
}
