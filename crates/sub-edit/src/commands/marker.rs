//! Marker commands: add, move, rename and remove.
//!
//! A [`Marker`] lives either on a sequence, in sequence time, or on a clip, in
//! that clip's source time (OTIO puts them in both places). One
//! [`MarkerTarget`] covers both, so the ruler and the inspector drive the same
//! three commands.
//!
//! Removing a marker returns an [`AddMarker`] carrying the whole marker and
//! the position it sat at, so undo restores its identifier, note and order
//! rather than something that merely looks the same.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::{ClipId, Marker, MarkerId, Project, SequenceId, TrackId};
use sub_time::TimeRange;

use super::{check_insert_index, clip_mut, sequence_mut};
use crate::{Command, Inverse, codes};

/// What a marker is anchored to.
///
/// The JSON shape is a tagged union, so an envelope reads
/// `{ "on": "sequence", "sequence": … }` or
/// `{ "on": "clip", "sequence": …, "track": …, "clip": … }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case", deny_unknown_fields)]
pub enum MarkerTarget {
    /// The sequence's own marker list, in sequence time.
    Sequence {
        /// The sequence carrying the markers.
        sequence: SequenceId,
    },
    /// A clip's marker list, in that clip's source time.
    Clip {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track holding the clip.
        track: TrackId,
        /// The clip carrying the markers.
        clip: ClipId,
    },
}

impl MarkerTarget {
    /// Markers on the sequence `sequence`.
    #[must_use]
    pub fn sequence(sequence: SequenceId) -> Self {
        Self::Sequence { sequence }
    }

    /// Markers on the clip `clip`.
    #[must_use]
    pub fn clip(sequence: SequenceId, track: TrackId, clip: ClipId) -> Self {
        Self::Clip {
            sequence,
            track,
            clip,
        }
    }

    /// The marker list this target names, mutably.
    ///
    /// A clip target goes through the clip-edit lookup, so markers on a locked
    /// track are as protected as the clips they annotate.
    ///
    /// # Errors
    ///
    /// - `edit.sequence_not_found`, `edit.track_not_found`, `edit.track_locked`
    ///   or `edit.clip_not_found` when the target does not resolve.
    fn markers_mut(self, project: &mut Project) -> SubResult<&mut Vec<Marker>> {
        match self {
            Self::Sequence { sequence } => Ok(&mut sequence_mut(project, sequence)?.markers),
            Self::Clip {
                sequence,
                track,
                clip,
            } => Ok(&mut clip_mut(project, sequence, track, clip)?.markers),
        }
    }

    /// The position of the marker with `id` in this target's list.
    ///
    /// # Errors
    ///
    /// Whatever [`MarkerTarget::markers_mut`] returns, or
    /// `edit.marker_not_found` when the target holds no such marker.
    fn index_of(self, project: &mut Project, id: MarkerId) -> SubResult<usize> {
        let markers = self.markers_mut(project)?;
        markers
            .iter()
            .position(|marker| marker.id == id)
            .ok_or_else(|| {
                SubError::new(codes::MARKER_NOT_FOUND, "no such marker")
                    .with_detail("marker_id", id)
            })
    }
}

/// Adds a marker to a sequence or a clip.
///
/// The marker arrives whole, with its own identifier, so replaying the command
/// from a log or a plugin reproduces the same project byte for byte. It is
/// appended unless `index` names a position.
///
/// ```
/// use sub_edit::History;
/// use sub_edit::commands::{AddMarker, MarkerTarget};
/// use sub_model::{Marker, Project, Sequence, SequenceSettings};
/// use sub_time::{Rational, RationalTime, TimeRange};
///
/// let mut project = Project::new("Doc cut");
/// let sequence = Sequence::new("Main", SequenceSettings::default());
/// let sequence_id = sequence.id;
/// project.sequences.push(sequence);
///
/// let at = RationalTime::new(48, Rational::FPS_24);
/// let marker = Marker::new("cut here", TimeRange::empty_at(at));
/// let mut history = History::new();
/// history
///     .apply(
///         &mut project,
///         AddMarker::new(MarkerTarget::sequence(sequence_id), marker),
///     )
///     .unwrap();
/// assert_eq!(project.sequences[0].markers.len(), 1);
///
/// history.undo(&mut project).unwrap();
/// assert!(project.sequences[0].markers.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddMarker {
    /// What the marker is anchored to.
    pub target: MarkerTarget,
    /// The marker itself.
    pub marker: Marker,
    /// Where in the target's marker list it goes. Appended when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

impl AddMarker {
    /// Appends `marker` to `target`.
    #[must_use]
    pub fn new(target: MarkerTarget, marker: Marker) -> Self {
        Self {
            target,
            marker,
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

impl Command for AddMarker {
    const KIND: &'static str = "marker.add";
    const DESCRIPTION: &'static str = "Add a marker to a sequence or a clip.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let id = self.marker.id;
        let markers = self.target.markers_mut(project)?;
        let index = self.index.unwrap_or(markers.len());
        check_insert_index(index, markers.len(), "marker")?;
        if markers.iter().any(|marker| marker.id == id) {
            return Err(SubError::new(
                codes::DUPLICATE_MARKER,
                "a marker with this identifier is already on the target",
            )
            .with_detail("marker_id", id));
        }

        markers.insert(index, self.marker.clone());
        Ok(Inverse::new(RemoveMarker::new(self.target, id)))
    }

    fn label(&self) -> String {
        format!("Add marker {}", self.marker.name)
    }
}

/// Removes a marker from a sequence or a clip.
///
/// The inverse is an [`AddMarker`] carrying the removed marker and its index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveMarker {
    /// What the marker is anchored to.
    pub target: MarkerTarget,
    /// The marker to remove.
    pub marker: MarkerId,
}

impl RemoveMarker {
    /// Removes the marker `marker` from `target`.
    #[must_use]
    pub fn new(target: MarkerTarget, marker: MarkerId) -> Self {
        Self { target, marker }
    }
}

impl Command for RemoveMarker {
    const KIND: &'static str = "marker.remove";
    const DESCRIPTION: &'static str = "Remove a marker from a sequence or a clip.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let index = self.target.index_of(project, self.marker)?;
        let removed = self.target.markers_mut(project)?.remove(index);
        Ok(Inverse::new(AddMarker::new(self.target, removed).at(index)))
    }

    fn label(&self) -> String {
        "Remove marker".to_owned()
    }
}

/// Moves a marker to another span, in the time of whatever holds it.
///
/// Dragging a point marker along the ruler is this command with an empty range
/// at the new instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveMarker {
    /// What the marker is anchored to.
    pub target: MarkerTarget,
    /// The marker to move.
    pub marker: MarkerId,
    /// The span it takes on.
    pub marked_range: TimeRange,
}

impl MoveMarker {
    /// Moves the marker `marker` on `target` to `marked_range`.
    #[must_use]
    pub fn new(target: MarkerTarget, marker: MarkerId, marked_range: TimeRange) -> Self {
        Self {
            target,
            marker,
            marked_range,
        }
    }
}

impl Command for MoveMarker {
    const KIND: &'static str = "marker.move";
    const DESCRIPTION: &'static str = "Move a marker to a different time.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let index = self.target.index_of(project, self.marker)?;
        let markers = self.target.markers_mut(project)?;
        let previous = std::mem::replace(&mut markers[index].marked_range, self.marked_range);
        Ok(Inverse::new(Self::new(self.target, self.marker, previous)))
    }

    fn label(&self) -> String {
        "Move marker".to_owned()
    }
}

/// Gives a marker a new name.
///
/// Renaming in place on the ruler is this command. The marker keeps its
/// identifier, its span and its note, so undo puts the old name back on the
/// same marker rather than on one that merely looks like it.
///
/// ```
/// use sub_edit::History;
/// use sub_edit::commands::{AddMarker, MarkerTarget, RenameMarker};
/// use sub_model::{Marker, Project, Sequence, SequenceSettings};
/// use sub_time::{Rational, RationalTime, TimeRange};
///
/// let mut project = Project::new("Doc cut");
/// let sequence = Sequence::new("Main", SequenceSettings::default());
/// let sequence_id = sequence.id;
/// project.sequences.push(sequence);
///
/// let at = RationalTime::new(48, Rational::FPS_24);
/// let marker = Marker::new("Marker", TimeRange::empty_at(at));
/// let marker_id = marker.id;
/// let target = MarkerTarget::sequence(sequence_id);
/// let mut history = History::new();
/// history
///     .apply(&mut project, AddMarker::new(target, marker))
///     .unwrap();
/// history
///     .apply(&mut project, RenameMarker::new(target, marker_id, "Reshoot"))
///     .unwrap();
/// assert_eq!(project.sequences[0].markers[0].name, "Reshoot");
///
/// history.undo(&mut project).unwrap();
/// assert_eq!(project.sequences[0].markers[0].name, "Marker");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameMarker {
    /// What the marker is anchored to.
    pub target: MarkerTarget,
    /// The marker to rename.
    pub marker: MarkerId,
    /// The name it takes on.
    pub name: String,
}

impl RenameMarker {
    /// Renames the marker `marker` on `target` to `name`.
    #[must_use]
    pub fn new(target: MarkerTarget, marker: MarkerId, name: impl Into<String>) -> Self {
        Self {
            target,
            marker,
            name: name.into(),
        }
    }
}

impl Command for RenameMarker {
    const KIND: &'static str = "marker.rename";
    const DESCRIPTION: &'static str = "Rename a marker.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let index = self.target.index_of(project, self.marker)?;
        let markers = self.target.markers_mut(project)?;
        let previous = std::mem::replace(&mut markers[index].name, self.name.clone());
        Ok(Inverse::new(Self::new(self.target, self.marker, previous)))
    }

    fn label(&self) -> String {
        format!("Rename marker to {}", self.name)
    }
}
