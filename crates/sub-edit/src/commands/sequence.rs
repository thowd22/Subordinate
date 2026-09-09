//! Sequence commands: create, insert, delete, rename and settings.
//!
//! Sequence order is the tab order, so creating and deleting address a
//! position as well as an identifier, and deleting a sequence returns an
//! inverse carrying the whole thing — tracks, clips and markers — so that
//! undoing a deletion is not a fresh empty timeline with the same name.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::{Project, Sequence, SequenceId, SequenceSettings};

use super::{check_insert_index, sequence_mut};
use crate::{Command, Inverse, codes};

/// Creates a new, empty sequence.
///
/// It is appended to the tab strip unless `index` names a position.
///
/// ```
/// use sub_edit::commands::CreateSequence;
/// use sub_edit::History;
/// use sub_model::{Project, SequenceSettings};
///
/// let mut project = Project::new("Doc cut");
/// let mut history = History::new();
/// history
///     .apply(&mut project, CreateSequence::new("Main", SequenceSettings::default()))
///     .unwrap();
/// assert_eq!(project.sequences.len(), 1);
///
/// history.undo(&mut project).unwrap();
/// assert!(project.sequences.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSequence {
    /// Display name, shown on the sequence tab.
    pub name: String,
    /// Canvas, timebase, audio rate and colour tags.
    pub settings: SequenceSettings,
    /// Where in [`Project::sequences`] it goes. Appended when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

impl CreateSequence {
    /// Appends a sequence named `name` with `settings`.
    #[must_use]
    pub fn new(name: impl Into<String>, settings: SequenceSettings) -> Self {
        Self {
            name: name.into(),
            settings,
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

impl Command for CreateSequence {
    const KIND: &'static str = "sequence.create";
    const DESCRIPTION: &'static str = "Create a new empty sequence with the given settings.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let index = self.index.unwrap_or(project.sequences.len());
        check_insert_index(index, project.sequences.len(), "sequence")?;

        let sequence = Sequence::new(self.name.clone(), self.settings);
        let id = sequence.id;
        project.sequences.insert(index, sequence);
        Ok(Inverse::new(DeleteSequence::new(id)))
    }

    fn label(&self) -> String {
        format!("Create sequence {}", self.name)
    }
}

/// Puts a whole sequence back at a given tab position.
///
/// This is the inverse of [`DeleteSequence`], and therefore what redo runs
/// after a [`CreateSequence`] is undone.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsertSequence {
    /// Where in [`Project::sequences`] it goes.
    pub index: usize,
    /// The sequence itself, exactly as it was.
    pub sequence: Sequence,
}

impl InsertSequence {
    /// Inserts `sequence` at `index`.
    #[must_use]
    pub fn new(index: usize, sequence: Sequence) -> Self {
        Self { index, sequence }
    }
}

impl Command for InsertSequence {
    const KIND: &'static str = "sequence.insert";
    const DESCRIPTION: &'static str =
        "Insert an existing sequence into the project at a given index.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        check_insert_index(self.index, project.sequences.len(), "sequence")?;
        let id = self.sequence.id;
        if project.sequence(id).is_some() {
            return Err(SubError::new(
                codes::DUPLICATE_SEQUENCE,
                "the project already holds a sequence with this identifier",
            )
            .with_detail("sequence_id", id));
        }

        project.sequences.insert(self.index, self.sequence.clone());
        Ok(Inverse::new(DeleteSequence::new(id)))
    }

    fn label(&self) -> String {
        format!("Restore sequence {}", self.sequence.name)
    }
}

/// Deletes a sequence and everything on it.
///
/// The inverse carries the whole sequence, so undo restores its tracks, clips,
/// markers and identifiers unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteSequence {
    /// The sequence to delete.
    pub sequence: SequenceId,
}

impl DeleteSequence {
    /// Deletes the sequence with `sequence`.
    #[must_use]
    pub fn new(sequence: SequenceId) -> Self {
        Self { sequence }
    }
}

impl Command for DeleteSequence {
    const KIND: &'static str = "sequence.delete";
    const DESCRIPTION: &'static str = "Delete a sequence and all of its tracks from the project.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let index = project.sequence_index(self.sequence).ok_or_else(|| {
            SubError::new(codes::SEQUENCE_NOT_FOUND, "no such sequence")
                .with_detail("sequence_id", self.sequence)
        })?;
        let sequence = project.sequences.remove(index);
        Ok(Inverse::new(InsertSequence::new(index, sequence)))
    }

    fn label(&self) -> String {
        "Delete sequence".to_owned()
    }
}

/// Renames a sequence.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameSequence {
    /// The sequence to rename.
    pub sequence: SequenceId,
    /// The new name.
    pub name: String,
}

impl RenameSequence {
    /// Renames `sequence` to `name`.
    #[must_use]
    pub fn new(sequence: SequenceId, name: impl Into<String>) -> Self {
        Self {
            sequence,
            name: name.into(),
        }
    }
}

impl Command for RenameSequence {
    const KIND: &'static str = "sequence.rename";
    const DESCRIPTION: &'static str = "Rename a sequence.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let found = sequence_mut(project, self.sequence)?;
        let previous = std::mem::replace(&mut found.name, self.name.clone());
        Ok(Inverse::new(Self::new(self.sequence, previous)))
    }

    fn label(&self) -> String {
        format!("Rename sequence to {}", self.name)
    }
}

/// Replaces a sequence's canvas, timebase, audio rate and colour tags.
///
/// [`SequenceSettings`] validates itself on the way in from JSON, so a command
/// carrying a zero sample rate or a zero canvas dimension is refused while it
/// is still an envelope, before it can touch the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetSequenceSettings {
    /// The sequence to reconfigure.
    pub sequence: SequenceId,
    /// The settings it takes on.
    pub settings: SequenceSettings,
}

impl SetSequenceSettings {
    /// Gives `sequence` the settings `settings`.
    #[must_use]
    pub fn new(sequence: SequenceId, settings: SequenceSettings) -> Self {
        Self { sequence, settings }
    }
}

impl Command for SetSequenceSettings {
    const KIND: &'static str = "sequence.set_settings";
    const DESCRIPTION: &'static str =
        "Replace a sequence's frame rate, resolution and colour settings.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let found = sequence_mut(project, self.sequence)?;
        let previous = std::mem::replace(&mut found.settings, self.settings);
        Ok(Inverse::new(Self::new(self.sequence, previous)))
    }

    fn label(&self) -> String {
        "Change sequence settings".to_owned()
    }
}
