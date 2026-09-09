//! The primitive clip edits: add, remove, move, trim, split and ripple delete.
//!
//! These are the commands the timeline UI, the Command API, agents and plugins
//! compose into everything else (docs/PLAN.md §2, §5.1). Every one of them is
//! a [`Command`], so every one of them undoes.
//!
//! # Placement
//!
//! A [`Track`] stores its items positionally, exactly as OpenTimelineIO does:
//! an item starts where the items before it end, and empty time is a [`Gap`].
//! These commands work on the equivalent *timeline* view — clip plus start —
//! and rebuild the positional list afterwards, inserting gaps for the holes
//! they leave. Two consequences are worth knowing:
//!
//! - Gap identity is not preserved. Gaps are derived from the holes between
//!   clips, so an edit rewrites them.
//! - Empty time after the last clip is not stored. A track ends where its last
//!   clip ends.
//!
//! # Overlap resolution: overwrite
//!
//! Placing or moving a clip never pushes its neighbours aside; it *overwrites*
//! the span it lands on. [`RippleDelete`] is the one command here that closes a
//! hole. For each existing clip on the target track, with `span` the span being
//! written:
//!
//! | Existing clip versus `span`             | Result                                            |
//! |-----------------------------------------|---------------------------------------------------|
//! | No overlap (butt-joined counts as none) | Untouched                                         |
//! | Fully covered by `span`                 | Removed                                           |
//! | Overlapped at its tail                  | Trimmed: its out point moves back to `span` start |
//! | Overlapped at its head                  | Trimmed: its in point moves up to `span` end      |
//! | Strictly containing `span`              | Split: the head keeps the identity, the tail is a new clip |
//!
//! Trimming a clip this way shortens its `source_range` from the corresponding
//! side, so the frames that survive stay on the timeline instants they were on
//! before. Fades are clamped to the shortened clip. A transition sitting inside
//! the overwritten span is removed along with the cut it blended.
//!
//! # Undo
//!
//! An overwrite can trim, remove and split several clips at once, so these
//! commands undo through [`RestoreTrackItems`], which carries the affected
//! tracks' item lists as they were. Restoring is exact — identifiers included —
//! and its own inverse is the same command carrying the lists it replaced, so
//! redo is exact too.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::{
    Clip, ClipId, Gap, MediaId, Project, Sequence, SequenceId, Track, TrackId, TrackItem,
};
use sub_time::{Rational, RationalTime, TimeRange};

use crate::codes;
use crate::command::{Command, CommandRegistry, Inverse};

/// Registers every clip command, and the restore command they undo through,
/// on `registry`.
///
/// # Errors
///
/// Returns `edit.duplicate_command` if one of the kinds is already registered.
pub fn register(registry: &mut CommandRegistry) -> SubResult<()> {
    registry.register::<AddClip>()?;
    registry.register::<RemoveClip>()?;
    registry.register::<MoveClip>()?;
    registry.register::<TrimClipIn>()?;
    registry.register::<TrimClipOut>()?;
    registry.register::<SplitClip>()?;
    registry.register::<RippleDelete>()?;
    registry.register::<RestoreTrackItems>()?;
    Ok(())
}

/// Puts a clip on a track at a timeline position, overwriting what is there.
///
/// The clip arrives whole, with its own identifier, so replaying the command
/// from a log or a plugin reproduces the same project byte for byte.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddClip {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track the clip lands on.
    pub track: TrackId,
    /// Where the clip starts, in sequence time.
    pub start: RationalTime,
    /// The clip itself.
    pub clip: Clip,
}

impl Command for AddClip {
    const KIND: &'static str = "clip.add";
    const DESCRIPTION: &'static str =
        "Place a clip on a track at a timeline position, overwriting what is there.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        require_media(project, self.clip.media)?;
        self.clip.validate()?;
        let span = placement_span(&self.clip, self.start)?;
        edit_track(project, self.sequence, self.track, |layout| {
            if layout.contains_clip(self.clip.id) {
                return Err(SubError::new(
                    codes::DUPLICATE_CLIP,
                    "a clip with that id is already on the track",
                )
                .with_detail("clip", self.clip.id));
            }
            layout.overwrite(span)?;
            layout.insert(self.start, TrackItem::Clip(self.clip.clone()));
            Ok(())
        })
    }

    fn label(&self) -> String {
        "Add clip".to_owned()
    }
}

/// Lifts a clip off its track, leaving the hole behind.
///
/// This is the non-rippling delete: the clips after it stay where they are.
/// Use [`RippleDelete`] to close the hole.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveClip {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip to remove.
    pub clip: ClipId,
}

impl Command for RemoveClip {
    const KIND: &'static str = "clip.remove";
    const DESCRIPTION: &'static str = "Lift a clip off its track, leaving the hole behind.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        edit_track(project, self.sequence, self.track, |layout| {
            layout.take_clip(self.clip)?;
            Ok(())
        })
    }

    fn label(&self) -> String {
        "Remove clip".to_owned()
    }
}

/// Moves a clip to another position, and optionally to another track.
///
/// The clip leaves a hole behind and overwrites what it lands on.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveClip {
    /// The sequence holding the tracks.
    pub sequence: SequenceId,
    /// The track the clip is on now.
    pub track: TrackId,
    /// The clip to move.
    pub clip: ClipId,
    /// Where the clip starts afterwards, in sequence time.
    pub start: RationalTime,
    /// The track the clip lands on; the same track when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_track: Option<TrackId>,
}

impl Command for MoveClip {
    const KIND: &'static str = "clip.move";
    const DESCRIPTION: &'static str =
        "Move a clip to another position or track, overwriting what is there.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let target = self.to_track.unwrap_or(self.track);
        if target == self.track {
            return edit_track(project, self.sequence, self.track, |layout| {
                let (_, clip) = layout.take_clip(self.clip)?;
                let span = placement_span(&clip, self.start)?;
                layout.overwrite(span)?;
                layout.insert(self.start, TrackItem::Clip(clip));
                Ok(())
            });
        }

        let rate = sequence_rate(project, self.sequence)?;
        let sequence = sequence_mut(project, self.sequence)?;
        let mut source = Layout::of_track(track_of(sequence, self.track)?, rate)?;
        let mut destination = Layout::of_track(track_of(sequence, target)?, rate)?;
        let (_, clip) = source.take_clip(self.clip)?;
        let span = placement_span(&clip, self.start)?;
        destination.overwrite(span)?;
        destination.insert(self.start, TrackItem::Clip(clip));
        let source_items = source.into_items()?;
        let destination_items = destination.into_items()?;
        validate_clips(&source_items)?;
        validate_clips(&destination_items)?;

        let source_previous =
            std::mem::replace(&mut track_mut(sequence, self.track)?.items, source_items);
        let destination_previous =
            std::mem::replace(&mut track_mut(sequence, target)?.items, destination_items);
        Ok(Inverse::new(RestoreTrackItems {
            sequence: self.sequence,
            tracks: vec![
                TrackItems {
                    track: self.track,
                    items: source_previous,
                },
                TrackItems {
                    track: target,
                    items: destination_previous,
                },
            ],
        }))
    }

    fn label(&self) -> String {
        "Move clip".to_owned()
    }
}

/// Moves a clip's in point, keeping its out point where it is.
///
/// A positive `delta` shortens the clip from the head and leaves a hole; a
/// negative one lengthens it, overwriting whatever lies before it.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrimClipIn {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip to trim.
    pub clip: ClipId,
    /// How far the in point moves later; negative moves it earlier.
    pub delta: RationalTime,
}

impl Command for TrimClipIn {
    const KIND: &'static str = "clip.trim_in";
    const DESCRIPTION: &'static str =
        "Move a clip's in point, changing where it starts on the timeline.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let limit = source_limit(project, clip_of(project, self)?.media);
        edit_track(project, self.sequence, self.track, |layout| {
            let (was_at, clip) = layout.take_clip(self.clip)?;
            let trimmed = trim_head(&clip, self.delta)?;
            check_source(&trimmed, limit)?;
            let start = checked(was_at.checked_add(self.delta), "trimmed clip start")?;
            let span = placement_span(&trimmed, start)?;
            layout.overwrite(span)?;
            layout.insert(start, TrackItem::Clip(trimmed));
            Ok(())
        })
    }

    fn label(&self) -> String {
        "Trim clip in".to_owned()
    }
}

/// Moves a clip's out point, keeping its in point where it is.
///
/// A positive `delta` lengthens the clip, overwriting whatever lies after it;
/// a negative one shortens it and leaves a hole.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrimClipOut {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip to trim.
    pub clip: ClipId,
    /// How far the out point moves later; negative moves it earlier.
    pub delta: RationalTime,
}

impl Command for TrimClipOut {
    const KIND: &'static str = "clip.trim_out";
    const DESCRIPTION: &'static str =
        "Move a clip's out point, changing where it ends on the timeline.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let limit = source_limit(project, clip_of(project, self)?.media);
        edit_track(project, self.sequence, self.track, |layout| {
            let (start, clip) = layout.take_clip(self.clip)?;
            let trimmed = trim_tail(&clip, self.delta)?;
            check_source(&trimmed, limit)?;
            let span = placement_span(&trimmed, start)?;
            layout.overwrite(span)?;
            layout.insert(start, TrackItem::Clip(trimmed));
            Ok(())
        })
    }

    fn label(&self) -> String {
        "Trim clip out".to_owned()
    }
}

/// Cuts a clip in two at a timeline instant.
///
/// The head keeps the clip's identity and the tail becomes a new clip butt
/// joined to it, so nothing on the track moves. `at` must fall strictly inside
/// the clip: splitting at either boundary, or outside the clip, returns
/// `edit.invalid_split` rather than silently doing nothing, because it almost
/// always means the caller aimed at the wrong clip or the wrong instant.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitClip {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip to split.
    pub clip: ClipId,
    /// Where to cut, in sequence time.
    pub at: RationalTime,
    /// The identity to give the tail; a fresh one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_id: Option<ClipId>,
}

impl Command for SplitClip {
    const KIND: &'static str = "clip.split";
    const DESCRIPTION: &'static str = "Split a clip in two at a timeline instant.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        edit_track(project, self.sequence, self.track, |layout| {
            let (start, clip) = layout.find_clip(self.clip)?;
            let range = placement_span(&clip, start)?;
            if !range.contains(self.at) || self.at == range.start() {
                return Err(SubError::new(
                    codes::INVALID_SPLIT,
                    "a split point must fall strictly inside the clip",
                )
                .with_detail("clip", self.clip)
                .with_detail("at", self.at.to_string())
                .with_detail("clip_range", range.to_string()));
            }
            let head_length = checked(self.at.checked_sub(start), "split offset")?;
            let tail_length = checked(range.end_exclusive().checked_sub(self.at), "split offset")?;
            let mut head = trim_tail(&clip, checked(tail_length.checked_neg(), "split offset")?)?;
            let mut tail = trim_head(&clip, head_length)?;
            // `ClipId::default` mints a fresh identifier.
            tail.id = self.tail_id.unwrap_or_default();
            if tail.id == head.id {
                return Err(SubError::new(
                    codes::DUPLICATE_CLIP,
                    "the tail of a split needs an identity of its own",
                )
                .with_detail("clip", self.clip));
            }
            split_fades(&clip, &mut head, &mut tail);
            layout.take_clip(self.clip)?;
            layout.insert(start, TrackItem::Clip(head));
            layout.insert(self.at, TrackItem::Clip(tail));
            Ok(())
        })
    }

    fn label(&self) -> String {
        "Split clip".to_owned()
    }
}

/// Removes a clip and closes the hole, pulling the rest of the track back.
///
/// Only the clip's own track ripples; other tracks keep their timing, so an
/// edit that must ripple several tracks issues one command per track inside a
/// history group.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RippleDelete {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip to remove.
    pub clip: ClipId,
}

impl Command for RippleDelete {
    const KIND: &'static str = "clip.ripple_delete";
    const DESCRIPTION: &'static str =
        "Remove a clip and close the hole by pulling later clips back.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        edit_track(project, self.sequence, self.track, |layout| {
            let (start, clip) = layout.take_clip(self.clip)?;
            layout.shift_from(
                start,
                checked(clip.duration().checked_neg(), "clip duration")?,
            )
        })
    }

    fn label(&self) -> String {
        "Ripple delete".to_owned()
    }
}

/// One track's items, as [`RestoreTrackItems`] carries them.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackItems {
    /// The track the items belong to.
    pub track: TrackId,
    /// The items, in playback order.
    pub items: Vec<TrackItem>,
}

/// Replaces whole tracks' item lists: the exact inverse every clip command
/// returns.
///
/// An overwrite or a ripple can touch any number of clips, so undo restores the
/// affected tracks wholesale rather than trying to invert each side effect one
/// by one. It is a command like any other: applying it returns the same command
/// carrying the lists it replaced, which is what redo applies.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreTrackItems {
    /// The sequence holding the tracks.
    pub sequence: SequenceId,
    /// The tracks to restore, in any order.
    pub tracks: Vec<TrackItems>,
}

impl Command for RestoreTrackItems {
    const KIND: &'static str = "edit.restore_track_items";
    const DESCRIPTION: &'static str =
        "Restore whole track item lists, the inverse the clip edits undo through.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let sequence = sequence_mut(project, self.sequence)?;
        // Validate everything before touching anything: the command is atomic.
        for entry in &self.tracks {
            track_of(sequence, entry.track)?;
            validate_clips(&entry.items)?;
        }
        let mut previous = Vec::with_capacity(self.tracks.len());
        for entry in &self.tracks {
            let track = track_mut(sequence, entry.track)?;
            previous.push(TrackItems {
                track: entry.track,
                items: std::mem::replace(&mut track.items, entry.items.clone()),
            });
        }
        Ok(Inverse::new(Self {
            sequence: self.sequence,
            tracks: previous,
        }))
    }

    fn label(&self) -> String {
        "Restore track".to_owned()
    }
}

/// The timeline view of a track: its clips and transitions, each with the
/// instant it starts at. Gaps are dropped on the way in and rebuilt from the
/// holes on the way out.
#[derive(Debug)]
struct Layout {
    /// The sequence timebase, the rate of the zero the walk starts from.
    rate: Rational,
    /// The placed items, in no particular order until [`Layout::into_items`]
    /// sorts them.
    placed: Vec<Placed>,
}

/// One item and the instant it starts at, in sequence time.
#[derive(Debug)]
struct Placed {
    /// Where the item starts.
    start: RationalTime,
    /// The item itself; never a [`Gap`].
    item: TrackItem,
}

impl Placed {
    /// Where this item ends. Transitions occupy no track time, so they end
    /// where they start.
    fn end(&self, rate: Rational) -> SubResult<RationalTime> {
        checked(
            self.start.checked_add(self.item.track_duration(rate)),
            "item end",
        )
    }

    /// How this item sorts against another starting at the same instant: a
    /// transition belongs at the cut, before the clip that follows it.
    fn rank(&self) -> u8 {
        u8::from(!matches!(self.item, TrackItem::Transition(_)))
    }
}

impl Layout {
    /// Reads a track into its timeline view.
    fn of_track(track: &Track, rate: Rational) -> SubResult<Self> {
        let mut cursor = RationalTime::zero(rate);
        let mut placed = Vec::with_capacity(track.items.len());
        for item in &track.items {
            let duration = item.track_duration(rate);
            if duration.is_negative() {
                return Err(SubError::new(
                    codes::INVALID_TIME,
                    "a track item lasts a negative time",
                )
                .with_detail("track", track.id)
                .with_detail("duration", duration.to_string()));
            }
            if !matches!(item, TrackItem::Gap(_)) {
                placed.push(Placed {
                    start: cursor,
                    item: item.clone(),
                });
            }
            cursor = checked(cursor.checked_add(duration), "track position")?;
        }
        Ok(Self { rate, placed })
    }

    /// Rebuilds the positional item list, filling the holes with gaps.
    fn into_items(mut self) -> SubResult<Vec<TrackItem>> {
        self.placed
            .sort_by_key(|placed| (placed.start, placed.rank()));
        let mut items = Vec::with_capacity(self.placed.len() * 2);
        let mut cursor = RationalTime::zero(self.rate);
        for placed in self.placed {
            let hole = checked(placed.start.checked_sub(cursor), "gap duration")?;
            if hole.is_negative() {
                return Err(SubError::new(
                    sub_core::codes::INVALID_STATE,
                    "two items would overlap on the same track",
                )
                .with_detail("start", placed.start.to_string()));
            }
            if !hole.is_zero() {
                items.push(TrackItem::Gap(Gap::new(hole)));
            }
            cursor = placed.end(self.rate)?;
            items.push(placed.item);
        }
        Ok(items)
    }

    /// The position of the clip with `id` in [`Layout::placed`].
    fn index_of(&self, id: ClipId) -> Option<usize> {
        self.placed
            .iter()
            .position(|placed| placed.item.as_clip().is_some_and(|clip| clip.id == id))
    }

    /// Whether a clip with `id` is on the track.
    fn contains_clip(&self, id: ClipId) -> bool {
        self.index_of(id).is_some()
    }

    /// The clip with `id` and where it starts, leaving it in place.
    fn find_clip(&self, id: ClipId) -> SubResult<(RationalTime, Clip)> {
        let index = self.index_of(id).ok_or_else(|| clip_not_found(id))?;
        let placed = &self.placed[index];
        let clip = placed
            .item
            .as_clip()
            .ok_or_else(|| clip_not_found(id))?
            .clone();
        Ok((placed.start, clip))
    }

    /// Takes the clip with `id` off the track and returns where it started.
    fn take_clip(&mut self, id: ClipId) -> SubResult<(RationalTime, Clip)> {
        let index = self.index_of(id).ok_or_else(|| clip_not_found(id))?;
        let placed = self.placed.remove(index);
        match placed.item {
            TrackItem::Clip(clip) => Ok((placed.start, clip)),
            _ => Err(clip_not_found(id)),
        }
    }

    /// Places an item at `start`.
    fn insert(&mut self, start: RationalTime, item: TrackItem) {
        self.placed.push(Placed { start, item });
    }

    /// Clears `span`, applying the overwrite policy documented on the module.
    fn overwrite(&mut self, span: TimeRange) -> SubResult<()> {
        if span.is_empty() {
            return Ok(());
        }
        let mut kept = Vec::with_capacity(self.placed.len() + 1);
        for placed in std::mem::take(&mut self.placed) {
            let clip = match placed.item {
                TrackItem::Transition(_) => {
                    // The cut this transition blended is being overwritten.
                    if !span.contains(placed.start) {
                        kept.push(placed);
                    }
                    continue;
                }
                TrackItem::Gap(_) => continue,
                TrackItem::Clip(clip) => clip,
            };
            let range = placement_span(&clip, placed.start)?;
            if !range.overlaps(span) {
                kept.push(Placed {
                    start: placed.start,
                    item: TrackItem::Clip(clip),
                });
                continue;
            }
            if span.contains_range(range) {
                continue;
            }
            let covers_head = range.start() < span.start();
            if covers_head {
                let overlap = checked(range.end_exclusive().checked_sub(span.start()), "overlap")?;
                let head = trim_tail(&clip, checked(overlap.checked_neg(), "overlap")?)?;
                kept.push(Placed {
                    start: range.start(),
                    item: TrackItem::Clip(head),
                });
            }
            if span.end_exclusive() < range.end_exclusive() {
                let overlap = checked(span.end_exclusive().checked_sub(range.start()), "overlap")?;
                let mut tail = trim_head(&clip, overlap)?;
                if covers_head {
                    // The span fell strictly inside the clip: the head kept the
                    // identity, so the tail needs one of its own.
                    tail.id = ClipId::new();
                }
                kept.push(Placed {
                    start: span.end_exclusive(),
                    item: TrackItem::Clip(tail),
                });
            }
        }
        self.placed = kept;
        Ok(())
    }

    /// Moves everything starting at or after `from` by `offset`.
    fn shift_from(&mut self, from: RationalTime, offset: RationalTime) -> SubResult<()> {
        for placed in &mut self.placed {
            if placed.start >= from {
                let start = checked(placed.start.checked_add(offset), "rippled position")?;
                if start.is_negative() {
                    return Err(SubError::new(
                        codes::INVALID_TIME,
                        "a ripple cannot move an item before the sequence start",
                    )
                    .with_detail("start", start.to_string()));
                }
                placed.start = start;
            }
        }
        Ok(())
    }
}

/// Runs `edit` over one track's timeline view and installs the result,
/// returning the inverse that restores the track.
///
/// The new item list is built and validated before anything is written, so a
/// failing edit leaves the project untouched.
fn edit_track<F>(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    edit: F,
) -> SubResult<Inverse>
where
    F: FnOnce(&mut Layout) -> SubResult<()>,
{
    let rate = sequence_rate(project, sequence_id)?;
    let sequence = sequence_mut(project, sequence_id)?;
    let mut layout = Layout::of_track(track_of(sequence, track_id)?, rate)?;
    edit(&mut layout)?;
    let items = layout.into_items()?;
    validate_clips(&items)?;
    let track = track_mut(sequence, track_id)?;
    let previous = std::mem::replace(&mut track.items, items);
    Ok(Inverse::new(RestoreTrackItems {
        sequence: sequence_id,
        tracks: vec![TrackItems {
            track: track_id,
            items: previous,
        }],
    }))
}

/// Checks every clip in an item list against [`Clip::validate`].
fn validate_clips(items: &[TrackItem]) -> SubResult<()> {
    for item in items {
        if let TrackItem::Clip(clip) = item {
            clip.validate()?;
        }
    }
    Ok(())
}

/// The span a clip occupies when it starts at `start`.
fn placement_span(clip: &Clip, start: RationalTime) -> SubResult<TimeRange> {
    if start.is_negative() {
        return Err(
            SubError::new(codes::INVALID_TIME, "a clip cannot start before zero")
                .with_detail("start", start.to_string()),
        );
    }
    TimeRange::new(start, clip.duration()).ok_or_else(|| {
        SubError::new(codes::INVALID_TIME, "clip placement is not representable")
            .with_detail("start", start.to_string())
            .with_detail("duration", clip.duration().to_string())
    })
}

/// Shortens a clip from the head by `delta`, or lengthens it when `delta` is
/// negative. The out point stays put.
fn trim_head(clip: &Clip, delta: RationalTime) -> SubResult<Clip> {
    let source = clip.source_range;
    let start = checked(source.start().checked_add(delta), "trimmed source start")?;
    let duration = checked(source.duration().checked_sub(delta), "trimmed duration")?;
    build_trimmed(clip, start, duration)
}

/// Lengthens a clip at the tail by `delta`, or shortens it when `delta` is
/// negative. The in point stays put.
fn trim_tail(clip: &Clip, delta: RationalTime) -> SubResult<Clip> {
    let source = clip.source_range;
    let duration = checked(source.duration().checked_add(delta), "trimmed duration")?;
    build_trimmed(clip, source.start(), duration)
}

/// Builds the trimmed clip, rejecting an empty or out-of-source result and
/// clamping the fades to what is left.
fn build_trimmed(clip: &Clip, start: RationalTime, duration: RationalTime) -> SubResult<Clip> {
    if start.is_negative() {
        return Err(SubError::new(
            codes::INVALID_TRIM,
            "a trim cannot reach before the start of the source",
        )
        .with_detail("clip", clip.id)
        .with_detail("source_start", start.to_string()));
    }
    if duration.is_negative() || duration.is_zero() {
        return Err(SubError::new(
            codes::INVALID_TRIM,
            "a trim must leave the clip lasting a positive time",
        )
        .with_detail("clip", clip.id)
        .with_detail("duration", duration.to_string()));
    }
    let source_range = TimeRange::new(start, duration).ok_or_else(|| {
        SubError::new(
            codes::INVALID_TRIM,
            "the trimmed source range is not representable",
        )
        .with_detail("clip", clip.id)
    })?;
    let mut trimmed = clip.clone();
    trimmed.source_range = source_range;
    clamp_fades(&mut trimmed);
    Ok(trimmed)
}

/// Shrinks the fades of a shortened clip so they still fit inside it.
fn clamp_fades(clip: &mut Clip) {
    let duration = clip.duration();
    if clip.fade_in > duration {
        clip.fade_in = duration;
    }
    let room = clip
        .fade_in
        .checked_neg()
        .and_then(|negated| duration.checked_add(negated))
        .unwrap_or_else(|| RationalTime::zero(duration.rate()));
    if clip.fade_out > room {
        clip.fade_out = room;
    }
}

/// Keeps the original fades on the outer edges of a split: the head keeps the
/// fade in, the tail keeps the fade out, and the new cut carries neither.
fn split_fades(original: &Clip, head: &mut Clip, tail: &mut Clip) {
    head.fade_in = original.fade_in;
    head.fade_out = RationalTime::zero(original.fade_out.rate());
    tail.fade_in = RationalTime::zero(original.fade_in.rate());
    tail.fade_out = original.fade_out;
    clamp_fades(head);
    clamp_fades(tail);
}

/// The end of the usable source for `media`, when the file has been probed.
fn source_limit(project: &Project, media: MediaId) -> Option<RationalTime> {
    project
        .media_item(media)?
        .info
        .as_ref()
        .and_then(|info| info.duration)
}

/// Rejects a trim that runs past the end of a probed source.
fn check_source(clip: &Clip, limit: Option<RationalTime>) -> SubResult<()> {
    let Some(limit) = limit else {
        return Ok(());
    };
    let end = clip.source_range.end_exclusive();
    if end > limit {
        return Err(SubError::new(
            codes::INVALID_TRIM,
            "a trim cannot reach past the end of the source",
        )
        .with_detail("clip", clip.id)
        .with_detail("source_end", end.to_string())
        .with_detail("source_duration", limit.to_string()));
    }
    Ok(())
}

/// Turns an exact-arithmetic failure into a structured error.
fn checked(value: Option<RationalTime>, what: &'static str) -> SubResult<RationalTime> {
    value.ok_or_else(|| {
        SubError::new(
            codes::INVALID_TIME,
            "the times involved cannot be combined exactly",
        )
        .with_detail("what", what)
    })
}

/// The `edit.clip_not_found` error for `id`.
fn clip_not_found(id: ClipId) -> SubError {
    SubError::new(codes::CLIP_NOT_FOUND, "no such clip on the track").with_detail("clip", id)
}

/// The `edit.sequence_not_found` error for `id`.
fn sequence_not_found(id: SequenceId) -> SubError {
    SubError::new(codes::SEQUENCE_NOT_FOUND, "no such sequence").with_detail("sequence", id)
}

/// The `edit.track_not_found` error for `id`.
fn track_not_found(id: TrackId) -> SubError {
    SubError::new(codes::TRACK_NOT_FOUND, "no such track in the sequence").with_detail("track", id)
}

/// The timebase of a sequence.
fn sequence_rate(project: &Project, id: SequenceId) -> SubResult<Rational> {
    project
        .sequence(id)
        .map(|sequence| sequence.settings.frame_rate)
        .ok_or_else(|| sequence_not_found(id))
}

/// The sequence with `id`, mutably.
fn sequence_mut(project: &mut Project, id: SequenceId) -> SubResult<&mut Sequence> {
    project
        .sequences
        .iter_mut()
        .find(|sequence| sequence.id == id)
        .ok_or_else(|| sequence_not_found(id))
}

/// The track with `id` in `sequence`.
fn track_of(sequence: &Sequence, id: TrackId) -> SubResult<&Track> {
    sequence.track(id).ok_or_else(|| track_not_found(id))
}

/// The track with `id` in `sequence`, mutably.
fn track_mut(sequence: &mut Sequence, id: TrackId) -> SubResult<&mut Track> {
    sequence
        .tracks
        .iter_mut()
        .find(|track| track.id == id)
        .ok_or_else(|| track_not_found(id))
}

/// Rejects a clip whose media is not in the project.
fn require_media(project: &Project, media: MediaId) -> SubResult<()> {
    if project.media_item(media).is_none() {
        return Err(
            SubError::new(codes::MEDIA_NOT_FOUND, "no such media item in the project")
                .with_detail("media", media),
        );
    }
    Ok(())
}

/// The clip a trim command names, looked up before the project is borrowed
/// mutably.
fn clip_of<'p, C: TrackClipCommand>(project: &'p Project, command: &C) -> SubResult<&'p Clip> {
    let sequence = project
        .sequence(command.sequence())
        .ok_or_else(|| sequence_not_found(command.sequence()))?;
    let track = track_of(sequence, command.track())?;
    track
        .clip(command.clip())
        .ok_or_else(|| clip_not_found(command.clip()))
}

/// What [`clip_of`] needs from a command that names one clip on one track.
trait TrackClipCommand {
    /// The sequence the command names.
    fn sequence(&self) -> SequenceId;
    /// The track the command names.
    fn track(&self) -> TrackId;
    /// The clip the command names.
    fn clip(&self) -> ClipId;
}

/// Implements [`TrackClipCommand`] for commands with the three usual fields.
macro_rules! impl_track_clip_command {
    ($($name:ident),+ $(,)?) => {
        $(impl TrackClipCommand for $name {
            fn sequence(&self) -> SequenceId {
                self.sequence
            }

            fn track(&self) -> TrackId {
                self.track
            }

            fn clip(&self) -> ClipId {
                self.clip
            }
        })+
    };
}

impl_track_clip_command!(TrimClipIn, TrimClipOut);
