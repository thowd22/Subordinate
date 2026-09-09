//! Track commands: add, insert, remove, reorder, rename, mute and lock.
//!
//! Track order is meaningful — video tracks composite top-down, so a later
//! entry renders over an earlier one — which is why every command here
//! addresses a position as well as an identifier, and why the inverse of a
//! removal puts the track back at the index it came from.

use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::{Project, SequenceId, Track, TrackId, TrackKind};

use super::{check_insert_index, sequence_mut, track_mut};
use crate::{Command, Inverse, codes};

/// Adds a new, empty track to a sequence.
///
/// The track is appended unless `index` names a position. Undo removes it;
/// redoing that undo goes through [`InsertTrack`], so the track comes back
/// with the identifier it was first given.
///
/// ```
/// use sub_edit::commands::AddTrack;
/// use sub_edit::{Command, History};
/// use sub_model::{Project, Sequence, SequenceSettings, TrackKind};
///
/// let mut project = Project::new("Doc cut");
/// let sequence = Sequence::new("Main", SequenceSettings::default());
/// let sequence_id = sequence.id;
/// project.sequences.push(sequence);
///
/// let mut history = History::new();
/// history
///     .apply(&mut project, AddTrack::new(sequence_id, "V1", TrackKind::Video))
///     .unwrap();
/// assert_eq!(project.sequences[0].tracks.len(), 1);
///
/// history.undo(&mut project).unwrap();
/// assert!(project.sequences[0].tracks.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddTrack {
    /// The sequence the track joins.
    pub sequence: SequenceId,
    /// Display name for the track header, such as `V1`.
    pub name: String,
    /// Whether the lane carries picture or sound.
    pub kind: TrackKind,
    /// Where in [`Sequence::tracks`](sub_model::Sequence::tracks) it goes.
    /// Appended when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

impl AddTrack {
    /// Appends a track of `kind` named `name` to `sequence`.
    #[must_use]
    pub fn new(sequence: SequenceId, name: impl Into<String>, kind: TrackKind) -> Self {
        Self {
            sequence,
            name: name.into(),
            kind,
            index: None,
        }
    }

    /// The same command, inserting at `index` instead of appending.
    #[must_use]
    pub fn at(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }
}

impl Command for AddTrack {
    const KIND: &'static str = "track.add";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let sequence = sequence_mut(project, self.sequence)?;
        let index = self.index.unwrap_or(sequence.tracks.len());
        check_insert_index(index, sequence.tracks.len(), "track")?;

        let track = Track::new(self.name.clone(), self.kind);
        let id = track.id;
        sequence.tracks.insert(index, track);
        Ok(Inverse::new(RemoveTrack::forced(self.sequence, id)))
    }

    fn label(&self) -> String {
        format!("Add track {}", self.name)
    }
}

/// Puts a whole track back at a given index.
///
/// This is the inverse of [`RemoveTrack`], and therefore what redo runs after
/// an [`AddTrack`] is undone. It carries the entire track — identifier, flags
/// and items — so undo is exact rather than approximate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsertTrack {
    /// The sequence the track joins.
    pub sequence: SequenceId,
    /// Where in [`Sequence::tracks`](sub_model::Sequence::tracks) it goes.
    pub index: usize,
    /// The track itself, exactly as it was.
    pub track: Track,
}

impl InsertTrack {
    /// Inserts `track` into `sequence` at `index`.
    #[must_use]
    pub fn new(sequence: SequenceId, index: usize, track: Track) -> Self {
        Self {
            sequence,
            index,
            track,
        }
    }
}

impl Command for InsertTrack {
    const KIND: &'static str = "track.insert";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let id = self.track.id;
        let sequence = sequence_mut(project, self.sequence)?;
        check_insert_index(self.index, sequence.tracks.len(), "track")?;
        if sequence.track(id).is_some() {
            return Err(SubError::new(
                codes::DUPLICATE_TRACK,
                "the sequence already holds a track with this identifier",
            )
            .with_detail("sequence_id", self.sequence)
            .with_detail("track_id", id));
        }

        sequence.tracks.insert(self.index, self.track.clone());
        Ok(Inverse::new(RemoveTrack::forced(self.sequence, id)))
    }

    fn label(&self) -> String {
        format!("Restore track {}", self.track.name)
    }
}

/// Removes a track from a sequence.
///
/// A track that still holds clips is refused with `edit.track_not_empty`
/// unless `force` is set: dropping an hour of cut material must be something
/// the caller asked for twice, not something a mis-click does. A forced
/// removal is undoable like any other command — the inverse carries the whole
/// track back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveTrack {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track to remove.
    pub track: TrackId,
    /// Whether to remove the track even though it still holds clips.
    #[serde(default)]
    pub force: bool,
}

impl RemoveTrack {
    /// Removes `track`, refusing to if it still holds clips.
    #[must_use]
    pub fn new(sequence: SequenceId, track: TrackId) -> Self {
        Self {
            sequence,
            track,
            force: false,
        }
    }

    /// Removes `track` and the clips on it.
    #[must_use]
    pub fn forced(sequence: SequenceId, track: TrackId) -> Self {
        Self {
            sequence,
            track,
            force: true,
        }
    }
}

impl Command for RemoveTrack {
    const KIND: &'static str = "track.remove";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let sequence_id = self.sequence;
        let sequence = sequence_mut(project, sequence_id)?;
        let index = sequence.track_index(self.track).ok_or_else(|| {
            SubError::new(codes::TRACK_NOT_FOUND, "no such track")
                .with_detail("sequence_id", sequence_id)
                .with_detail("track_id", self.track)
        })?;

        let clips = sequence.tracks[index].clips().count();
        if clips > 0 && !self.force {
            return Err(SubError::new(
                codes::TRACK_NOT_EMPTY,
                "the track still holds clips; set force to remove it with them",
            )
            .with_detail("sequence_id", sequence_id)
            .with_detail("track_id", self.track)
            .with_detail("clips", clips));
        }

        let track = sequence.tracks.remove(index);
        Ok(Inverse::new(InsertTrack::new(sequence_id, index, track)))
    }

    fn label(&self) -> String {
        "Remove track".to_owned()
    }
}

/// Moves a track to another position in the stack.
///
/// The index is the one the track ends up at, counted in the list *without*
/// the track in it, which is what a drag in the track header does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReorderTrack {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track to move.
    pub track: TrackId,
    /// The position it ends up at.
    pub to_index: usize,
}

impl ReorderTrack {
    /// Moves `track` to `to_index`.
    #[must_use]
    pub fn new(sequence: SequenceId, track: TrackId, to_index: usize) -> Self {
        Self {
            sequence,
            track,
            to_index,
        }
    }
}

impl Command for ReorderTrack {
    const KIND: &'static str = "track.reorder";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let sequence_id = self.sequence;
        let sequence = sequence_mut(project, sequence_id)?;
        let from = sequence.track_index(self.track).ok_or_else(|| {
            SubError::new(codes::TRACK_NOT_FOUND, "no such track")
                .with_detail("sequence_id", sequence_id)
                .with_detail("track_id", self.track)
        })?;
        check_insert_index(self.to_index, sequence.tracks.len() - 1, "track")?;

        if from != self.to_index {
            let track = sequence.tracks.remove(from);
            sequence.tracks.insert(self.to_index, track);
        }
        Ok(Inverse::new(Self::new(sequence_id, self.track, from)))
    }

    fn label(&self) -> String {
        "Reorder track".to_owned()
    }
}

/// Renames a track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameTrack {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track to rename.
    pub track: TrackId,
    /// The new name.
    pub name: String,
}

impl RenameTrack {
    /// Renames `track` to `name`.
    #[must_use]
    pub fn new(sequence: SequenceId, track: TrackId, name: impl Into<String>) -> Self {
        Self {
            sequence,
            track,
            name: name.into(),
        }
    }
}

impl Command for RenameTrack {
    const KIND: &'static str = "track.rename";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let found = track_mut(project, self.sequence, self.track)?;
        let previous = std::mem::replace(&mut found.name, self.name.clone());
        Ok(Inverse::new(Self::new(self.sequence, self.track, previous)))
    }

    fn label(&self) -> String {
        format!("Rename track to {}", self.name)
    }
}

/// Mutes or unmutes a track.
///
/// A muted audio track contributes nothing to the mixer and a muted video
/// track nothing to the composite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetTrackMuted {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track to mute or unmute.
    pub track: TrackId,
    /// The new state.
    pub muted: bool,
}

impl SetTrackMuted {
    /// Sets the mute flag of `track`.
    #[must_use]
    pub fn new(sequence: SequenceId, track: TrackId, muted: bool) -> Self {
        Self {
            sequence,
            track,
            muted,
        }
    }
}

impl Command for SetTrackMuted {
    const KIND: &'static str = "track.set_muted";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let found = track_mut(project, self.sequence, self.track)?;
        let previous = std::mem::replace(&mut found.muted, self.muted);
        Ok(Inverse::new(Self::new(self.sequence, self.track, previous)))
    }

    fn label(&self) -> String {
        if self.muted {
            "Mute track".to_owned()
        } else {
            "Unmute track".to_owned()
        }
    }
}

/// Locks or unlocks a track.
///
/// Clip commands refuse to touch a locked track — see
/// [`track_for_clip_edit`](super::track_for_clip_edit) — but this command
/// reaches it either way, or a locked track could never be unlocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetTrackLocked {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track to lock or unlock.
    pub track: TrackId,
    /// The new state.
    pub locked: bool,
}

impl SetTrackLocked {
    /// Sets the lock flag of `track`.
    #[must_use]
    pub fn new(sequence: SequenceId, track: TrackId, locked: bool) -> Self {
        Self {
            sequence,
            track,
            locked,
        }
    }
}

impl Command for SetTrackLocked {
    const KIND: &'static str = "track.set_locked";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let found = track_mut(project, self.sequence, self.track)?;
        let previous = std::mem::replace(&mut found.locked, self.locked);
        Ok(Inverse::new(Self::new(self.sequence, self.track, previous)))
    }

    fn label(&self) -> String {
        if self.locked {
            "Lock track".to_owned()
        } else {
            "Unlock track".to_owned()
        }
    }
}
