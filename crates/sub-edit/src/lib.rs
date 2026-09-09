//! Editing operations as undoable commands.
//!
//! Every mutation of a project is a [`Command`] with apply and revert. The
//! same command set is what the Command API, the MCP server and plugins
//! expose, so no editing logic may bypass this crate. See docs/PLAN.md §5.1
//! and decision-7.
//!
//! This crate is the engine underneath that rule:
//!
//! - [`Command`] — a serde type that applies itself to a `Project` and returns
//!   the [`Inverse`] that undoes it.
//! - [`CommandEnvelope`] and [`CommandRegistry`] — the stable
//!   `{ "kind": …, "params": … }` JSON a command travels and is logged as, and
//!   the map that turns it back into something applicable.
//! - [`History`] — the undo/redo stack, with a configurable depth and a
//!   grouping API so a drag that moves twelve clips undoes in one step.
//!
//! - [`clip`] — the primitive clip edits: add, remove, move, trim in and out,
//!   split and ripple delete, with the overwrite overlap policy they share.
//!
//! The remaining commands live in the tasks that follow: track and sequence
//! commands, parameter, marker, media and bin commands. They all implement
//! [`Command`] and register their [`Command::KIND`] here.
//!
//! ```
//! use serde::{Deserialize, Serialize};
//! use sub_core::SubResult;
//! use sub_edit::{Command, History, Inverse};
//! use sub_model::{Project, json};
//!
//! #[derive(Debug, Serialize, Deserialize)]
//! struct RenameProject {
//!     name: String,
//! }
//!
//! impl Command for RenameProject {
//!     const KIND: &'static str = "project.rename";
//!
//!     fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
//!         let previous = std::mem::replace(&mut project.name, self.name.clone());
//!         Ok(Inverse::new(RenameProject { name: previous }))
//!     }
//!
//!     fn label(&self) -> String {
//!         "Rename project".to_owned()
//!     }
//! }
//!
//! let mut project = Project::new("Untitled");
//! let before = json::to_json(&project).unwrap();
//! let mut history = History::new();
//!
//! history
//!     .apply(&mut project, RenameProject { name: "Doc cut".to_owned() })
//!     .unwrap();
//! let after = json::to_json(&project).unwrap();
//!
//! assert_eq!(history.undo_label(), Some("Rename project"));
//! history.undo(&mut project).unwrap();
//! assert_eq!(json::to_json(&project).unwrap(), before);
//! history.redo(&mut project).unwrap();
//! assert_eq!(json::to_json(&project).unwrap(), after);
//! ```

pub mod clip;
pub mod command;
pub mod history;

pub use clip::{
    AddClip, MoveClip, RemoveClip, RestoreTrackItems, RippleDelete, SplitClip, TrackItems,
    TrimClipIn, TrimClipOut,
};
pub use command::{AnyCommand, BoxedCommand, Command, CommandEnvelope, CommandRegistry, Inverse};
pub use history::{DEFAULT_DEPTH, History, HistoryEntry};

/// The error codes this crate produces.
///
/// Codes are part of the public contract with agents and plugins: an existing
/// one is never renamed or given a new meaning (see `sub_core::error`).
pub mod codes {
    use sub_core::ErrorCode;

    /// A command envelope names a kind this build does not know.
    pub const UNKNOWN_COMMAND: ErrorCode = ErrorCode::from_static("edit.unknown_command");
    /// A command's parameters do not match the command, or the command could
    /// not be serialised.
    pub const INVALID_COMMAND: ErrorCode = ErrorCode::from_static("edit.invalid_command");
    /// Two commands claim the same wire kind.
    pub const DUPLICATE_COMMAND: ErrorCode = ErrorCode::from_static("edit.duplicate_command");
    /// A command group is open and the requested operation would interleave
    /// with it.
    pub const GROUP_OPEN: ErrorCode = ErrorCode::from_static("edit.group_open");
    /// A group was committed or aborted while none was open.
    pub const NO_GROUP: ErrorCode = ErrorCode::from_static("edit.no_group");
    /// A command names a sequence the project does not hold.
    pub const SEQUENCE_NOT_FOUND: ErrorCode = ErrorCode::from_static("edit.sequence_not_found");
    /// A command names a track the sequence does not hold.
    pub const TRACK_NOT_FOUND: ErrorCode = ErrorCode::from_static("edit.track_not_found");
    /// A command names a clip the track does not hold.
    pub const CLIP_NOT_FOUND: ErrorCode = ErrorCode::from_static("edit.clip_not_found");
    /// A command names a media item the project does not hold.
    pub const MEDIA_NOT_FOUND: ErrorCode = ErrorCode::from_static("edit.media_not_found");
    /// A clip identity would appear twice on the same track.
    pub const DUPLICATE_CLIP: ErrorCode = ErrorCode::from_static("edit.duplicate_clip");
    /// A split point falls on a clip boundary or outside the clip.
    pub const INVALID_SPLIT: ErrorCode = ErrorCode::from_static("edit.invalid_split");
    /// A trim would empty a clip, or reach outside its source media.
    pub const INVALID_TRIM: ErrorCode = ErrorCode::from_static("edit.invalid_trim");
    /// A position or duration is negative, or cannot be combined exactly with
    /// the others involved.
    pub const INVALID_TIME: ErrorCode = ErrorCode::from_static("edit.invalid_time");
}

/// Small commands the unit tests apply to a project.
///
/// These exist so the trait, the registry and the history can be tested
/// without the real command set.
#[cfg(test)]
mod test_commands {
    use serde::{Deserialize, Serialize};
    use sub_core::{ErrorCode, SubError, SubResult};
    use sub_model::{Project, Sequence, SequenceSettings};

    use crate::{Command, Inverse};

    /// The code [`Failing`] returns.
    pub const FAILED: ErrorCode = ErrorCode::from_static("test.failed");

    /// Sets the project name.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct SetName {
        pub name: String,
    }

    impl SetName {
        pub fn new(name: impl Into<String>) -> Self {
            Self { name: name.into() }
        }
    }

    impl Command for SetName {
        const KIND: &'static str = "test.set_name";

        fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
            let previous = std::mem::replace(&mut project.name, self.name.clone());
            Ok(Inverse::new(Self::new(previous)))
        }
    }

    /// The same mutation with a human label, to prove labels reach the menu.
    #[derive(Debug, Serialize, Deserialize)]
    pub struct Rename {
        pub name: String,
    }

    impl Rename {
        pub fn new(name: impl Into<String>) -> Self {
            Self { name: name.into() }
        }
    }

    impl Command for Rename {
        const KIND: &'static str = "test.rename";

        fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
            let previous = std::mem::replace(&mut project.name, self.name.clone());
            Ok(Inverse::new(Self::new(previous)))
        }

        fn label(&self) -> String {
            "Rename project".to_owned()
        }
    }

    /// Appends a sequence; its inverse removes the last one.
    #[derive(Debug, Serialize, Deserialize)]
    pub struct AddSequence {
        pub name: String,
    }

    impl AddSequence {
        pub fn new(name: impl Into<String>) -> Self {
            Self { name: name.into() }
        }
    }

    impl Command for AddSequence {
        const KIND: &'static str = "test.add_sequence";

        fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
            project.sequences.push(Sequence::new(
                self.name.clone(),
                SequenceSettings::default(),
            ));
            Ok(Inverse::new(RemoveLastSequence {}))
        }
    }

    /// The inverse of [`AddSequence`].
    #[derive(Debug, Serialize, Deserialize)]
    pub struct RemoveLastSequence {}

    impl Command for RemoveLastSequence {
        const KIND: &'static str = "test.remove_last_sequence";

        fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
            let sequence = project.sequences.pop().ok_or_else(|| {
                SubError::new(sub_core::codes::INVALID_STATE, "no sequence to remove")
            })?;
            Ok(Inverse::new(AddSequence::new(sequence.name)))
        }
    }

    /// A command that always fails without touching the project.
    #[derive(Debug, Serialize, Deserialize)]
    pub struct Failing {}

    impl Failing {
        pub fn new() -> Self {
            Self {}
        }
    }

    impl Command for Failing {
        const KIND: &'static str = "test.failing";

        fn apply(&self, _project: &mut Project) -> SubResult<Inverse> {
            Err(SubError::new(FAILED, "this command always fails"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_live_in_the_edit_domain() {
        for code in [
            codes::UNKNOWN_COMMAND,
            codes::INVALID_COMMAND,
            codes::DUPLICATE_COMMAND,
            codes::GROUP_OPEN,
            codes::NO_GROUP,
            codes::SEQUENCE_NOT_FOUND,
            codes::TRACK_NOT_FOUND,
            codes::CLIP_NOT_FOUND,
            codes::MEDIA_NOT_FOUND,
            codes::DUPLICATE_CLIP,
            codes::INVALID_SPLIT,
            codes::INVALID_TRIM,
            codes::INVALID_TIME,
        ] {
            assert_eq!(code.domain(), "edit");
            assert!(sub_core::ErrorCode::parse(code.as_str()).is_ok());
        }
    }
}
