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
//! - [`commands`] — the rest of the set: track and sequence commands, clip
//!   parameters, markers, media items and bins, with [`register_builtin`] to
//!   put every one of them on a registry.
//! - [`Engine`] — the thread that owns the project: commands go in on a queue,
//!   readers take immutable [`ChangeEvent`]-announced snapshots out, and the
//!   [`EventBus`] broadcasts what changed.
//! - [`autosave`] — the background snapshot history in the project's sidecar
//!   directory, and the recovery check an open makes against it.
//! - [`playback`] — the playback scheduler and its clock: play, pause, JKL
//!   shuttling, a loop range, and the frame dropping that keeps playback in
//!   time when decode falls behind.
//! - [`relink`] — finding a moved source file by content hash, then by name,
//!   and relinking every offline item it accounts for as one undo step.
//!
//! ```
//! use schemars::JsonSchema;
//! use serde::{Deserialize, Serialize};
//! use sub_core::SubResult;
//! use sub_edit::{Command, History, Inverse};
//! use sub_model::{Project, json};
//!
//! #[derive(Debug, JsonSchema, Serialize, Deserialize)]
//! struct RenameProject {
//!     name: String,
//! }
//!
//! impl Command for RenameProject {
//!     const KIND: &'static str = "project.rename";
//!     const DESCRIPTION: &'static str = "Rename the project.";
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

pub mod autosave;
pub mod bus;
pub mod clip;
pub mod command;
pub mod commands;
pub mod engine;
pub mod event;
pub mod history;
pub mod playback;
pub mod relink;

pub use autosave::{
    Autosave, AutosaveConfig, AutosaveStatus, Recovery, Snapshot, SnapshotStore, check_for_recovery,
};
pub use bus::{DEFAULT_EVENT_CAPACITY, EventBus, EventReceiver};
pub use clip::{
    AddClip, InsertClip, MoveClip, RemoveClip, RestoreTrackItems, RippleDelete, SplitClip,
    TrackItems, TrimClipIn, TrimClipOut,
};
pub use command::{AnyCommand, BoxedCommand, Command, CommandEnvelope, CommandRegistry, Inverse};
pub use commands::{builtin_registry, register_builtin};
pub use engine::{
    Applied, Engine, EngineConfig, EngineHandle, HistorySummary, PlaybackOp, PlaybackStatus,
};
pub use event::{ChangeEvent, ChangeOrigin, ChangeType, EntityKind};
pub use history::{DEFAULT_DEPTH, History, HistoryEntry};
pub use playback::{MonotonicClock, PlaybackScheduler, PlayheadEvent, ShuttleSpeed, Tick};
pub use relink::{
    MatchKind, RelinkMatch, RelinkPlan, RelinkTarget, SearchOptions, match_chosen, match_offline,
    match_targets, scan_folder,
};

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
    /// A crossfade was asked for where one cannot go: a duration that is not
    /// positive, a cut with no clip on both sides, or a cut with no handle to
    /// blend across.
    pub const INVALID_TRANSITION: ErrorCode = ErrorCode::from_static("edit.invalid_transition");
    /// A command names a transition the track does not hold at that cut.
    pub const TRANSITION_NOT_FOUND: ErrorCode = ErrorCode::from_static("edit.transition_not_found");
    /// A clip command was aimed at a locked track.
    pub const TRACK_LOCKED: ErrorCode = ErrorCode::from_static("edit.track_locked");
    /// A track still holding clips was removed without `force`.
    pub const TRACK_NOT_EMPTY: ErrorCode = ErrorCode::from_static("edit.track_not_empty");
    /// A command names a position past the end of the list it inserts into.
    pub const INVALID_INDEX: ErrorCode = ErrorCode::from_static("edit.invalid_index");
    /// A track would be inserted with an identifier the sequence already uses.
    pub const DUPLICATE_TRACK: ErrorCode = ErrorCode::from_static("edit.duplicate_track");
    /// A sequence would be inserted with an identifier the project already
    /// uses.
    pub const DUPLICATE_SEQUENCE: ErrorCode = ErrorCode::from_static("edit.duplicate_sequence");
    /// A command names an effect the clip does not hold.
    pub const EFFECT_NOT_FOUND: ErrorCode = ErrorCode::from_static("edit.effect_not_found");
    /// An effect would be added to a clip that already holds one with that
    /// identifier.
    pub const DUPLICATE_EFFECT: ErrorCode = ErrorCode::from_static("edit.duplicate_effect");
    /// A command names a marker the sequence or clip does not hold.
    pub const MARKER_NOT_FOUND: ErrorCode = ErrorCode::from_static("edit.marker_not_found");
    /// A media item holds no findings from the analyzer a command named.
    pub const ANALYSIS_NOT_FOUND: ErrorCode = ErrorCode::from_static("edit.analysis_not_found");
    /// A command would leave a media item holding two analyses from one
    /// analyzer.
    pub const DUPLICATE_ANALYSIS: ErrorCode = ErrorCode::from_static("edit.duplicate_analysis");
    /// A marker would be added with an identifier its holder already uses.
    pub const DUPLICATE_MARKER: ErrorCode = ErrorCode::from_static("edit.duplicate_marker");
    /// A media item would be imported with an identifier the project already
    /// uses.
    pub const DUPLICATE_MEDIA: ErrorCode = ErrorCode::from_static("edit.duplicate_media");
    /// A media item still used by clips was removed without `force`.
    pub const MEDIA_IN_USE: ErrorCode = ErrorCode::from_static("edit.media_in_use");
    /// A command names a bin the project does not hold.
    pub const BIN_NOT_FOUND: ErrorCode = ErrorCode::from_static("edit.bin_not_found");
    /// A bin would be inserted with an identifier the bin tree already uses.
    pub const DUPLICATE_BIN: ErrorCode = ErrorCode::from_static("edit.duplicate_bin");
    /// A bin still holding media or child bins was removed without `force`.
    pub const BIN_NOT_EMPTY: ErrorCode = ErrorCode::from_static("edit.bin_not_empty");
    /// The root bin was removed, moved or made a child of itself.
    pub const ROOT_BIN: ErrorCode = ErrorCode::from_static("edit.root_bin");
    /// A command was submitted to an engine whose thread has stopped.
    pub const ENGINE_STOPPED: ErrorCode = ErrorCode::from_static("edit.engine_stopped");
    /// An autosave snapshot could not be written, listed, read or removed.
    pub const AUTOSAVE_FAILED: ErrorCode = ErrorCode::from_static("edit.autosave_failed");
    /// A snapshot named for restoring is no longer on disk.
    pub const SNAPSHOT_NOT_FOUND: ErrorCode = ErrorCode::from_static("edit.snapshot_not_found");
}

/// Small commands the unit tests apply to a project.
///
/// These exist so the trait, the registry and the history can be tested
/// without the real command set.
#[cfg(test)]
mod test_commands {
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use sub_core::{ErrorCode, SubError, SubResult};
    use sub_model::{Project, Sequence, SequenceSettings};

    use crate::{Command, Inverse};

    /// The code [`Failing`] returns.
    pub const FAILED: ErrorCode = ErrorCode::from_static("test.failed");

    /// Sets the project name.
    #[derive(Debug, JsonSchema, Serialize, Deserialize)]
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
    #[derive(Debug, JsonSchema, Serialize, Deserialize)]
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
    #[derive(Debug, JsonSchema, Serialize, Deserialize)]
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
    #[derive(Debug, JsonSchema, Serialize, Deserialize)]
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
    #[derive(Debug, JsonSchema, Serialize, Deserialize)]
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
            codes::TRACK_LOCKED,
            codes::TRACK_NOT_EMPTY,
            codes::INVALID_INDEX,
            codes::DUPLICATE_TRACK,
            codes::EFFECT_NOT_FOUND,
            codes::DUPLICATE_EFFECT,
            codes::DUPLICATE_SEQUENCE,
            codes::ENGINE_STOPPED,
        ] {
            assert_eq!(code.domain(), "edit");
            assert!(sub_core::ErrorCode::parse(code.as_str()).is_ok());
        }
    }
}
