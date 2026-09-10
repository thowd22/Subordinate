//! WASM plugin host on wasmtime: manifests, capabilities, WIT worlds and hot
//! reload.
//!
//! Plugins are sandboxed WebAssembly components with versioned WIT
//! interfaces. Fuel and epoch limits keep a misbehaving plugin from stalling
//! the engine. See docs/PLAN.md §6.
//!
//! # The host interface
//!
//! `wit/subordinate-plugin.wit` declares `subordinate:plugin@0.1.0`. Every
//! plugin world imports the one `command-api` interface, which is the
//! plugin-side view of the Command API (decision-6, decision-7):
//!
//! - `run-command(project, method, params)` — a JSON-RPC method name and its
//!   `params` object, exactly what the local socket server takes, so a plugin
//!   and an external agent have the same reach. Each success is one undoable
//!   command on the host's stack.
//! - `query(project, method, params)` — the read-only half, same encoding.
//! - `open-projects`, `project-info`, `sequences`, `tracks`, `clips`,
//!   `markers`, `playhead` — the
//!   metadata accessors, which return records rather than JSON because their
//!   shape is fixed and a plugin should not have to parse to learn the
//!   timebase it must respect.
//! - `log(level, message)` — a line into the host's `tracing` subscriber,
//!   tagged with the plugin id.
//!
//! # Plugin-contributed commands
//!
//! The `commands` world is the same import with menu and shortcut
//! registration on top: a plugin answers `commands()` once with a
//! [`CommandDesc`] per entry it contributes, and the host validates and keeps
//! those in a [`PluginCommandRegistry`], which is what the Plugins menu and the
//! keyboard map read. [`run_as_undo_group`] then wraps a whole run in one
//! engine command group, so a plugin command is one step on the undo stack
//! however many primitives it applies, and a failed run leaves nothing behind.
//! See [`menu`].
//!
//! [`bindings`] holds the generated host side of that world; [`convert`] maps
//! its records onto [`sub_core::SubError`], [`sub_time::RationalTime`] and the
//! `sub_model` identifiers, so an implementation of the generated `Host` trait
//! (TASK-84) never hand-builds a record.
//!
//! ```
//! use sub_plugin::{WitError, WitRationalTime};
//! use sub_core::{ErrorCode, SubError};
//! use sub_time::{Rational, RationalTime};
//!
//! // A host error crossing out to a plugin keeps its stable code.
//! let error = SubError::new(ErrorCode::from_static("plugin.no_such_project"), "gone");
//! assert_eq!(WitError::from(&error).code, "plugin.no_such_project");
//!
//! // A playhead crosses as an exact fraction, never as seconds.
//! let time = RationalTime::from_frames(1, Rational::FPS_29_97);
//! assert_eq!(RationalTime::try_from(WitRationalTime::from(time)).unwrap(), time);
//! ```

pub mod bindings;
mod convert;
pub mod menu;

pub use bindings::Command;
pub use bindings::menu::Commands;
pub use bindings::menu::subordinate::plugin::command_menu;
pub use bindings::subordinate::plugin::command_api;
pub use bindings::subordinate::plugin::command_api::{
    ClipMetadata, LogLevel, MarkerMetadata, ProjectMetadata, Resolution as WitResolution,
    SequenceMetadata, TrackKind as WitTrackKind, TrackMetadata,
};
pub use bindings::subordinate::plugin::types;
pub use bindings::subordinate::plugin::types::{
    ClipId as WitClipId, Detail as WitDetail, Error as WitError, MarkerId as WitMarkerId,
    MediaId as WitMediaId, ProjectId as WitProjectId, Rational as WitRational,
    RationalTime as WitRationalTime, SequenceId as WitSequenceId, TimeRange as WitTimeRange,
    TrackId as WitTrackId,
};
pub use convert::{
    marker_metadata, project_metadata, sequence_metadata, track_clip_metadata, track_metadata,
};
pub use menu::{
    CommandContext, CommandDesc, PluginCommand, PluginCommandRegistry, QUALIFIED_SEPARATOR,
    run_as_undo_group,
};

/// Error codes this crate raises. The `plugin.*` domain belongs to the host;
/// these are the ones the boundary itself can produce, before any plugin code
/// runs.
pub mod codes {
    use sub_core::ErrorCode;

    /// A plugin returned an error whose `code` is not a valid
    /// [`ErrorCode`](sub_core::ErrorCode). The original text is kept in the
    /// `plugin_code` detail.
    pub const INVALID_ERROR_CODE: ErrorCode = ErrorCode::from_static("plugin.invalid_error_code");
    /// A rate crossing in from a plugin has a zero numerator or denominator.
    pub const INVALID_RATIONAL: ErrorCode = ErrorCode::from_static("plugin.invalid_rational");
    /// A time range crossing in from a plugin has a negative duration or two
    /// endpoints at different rates.
    pub const INVALID_TIME_RANGE: ErrorCode = ErrorCode::from_static("plugin.invalid_time_range");
    /// A plugin registering commands has an id that is not a lowercase
    /// dot-separated identifier.
    pub const INVALID_PLUGIN_ID: ErrorCode = ErrorCode::from_static("plugin.invalid_plugin_id");
    /// A registered command's id is not a lowercase dot-separated identifier.
    pub const INVALID_COMMAND_ID: ErrorCode = ErrorCode::from_static("plugin.invalid_command_id");
    /// A registered command's title is empty or spans more than one line.
    pub const INVALID_COMMAND_TITLE: ErrorCode =
        ErrorCode::from_static("plugin.invalid_command_title");
    /// A registered command asks for a shortcut that is empty. An unset
    /// shortcut is how a command says it wants none.
    pub const INVALID_SHORTCUT: ErrorCode = ErrorCode::from_static("plugin.invalid_shortcut");
    /// One plugin registered the same command id twice.
    pub const DUPLICATE_COMMAND: ErrorCode = ErrorCode::from_static("plugin.duplicate_command");
    /// A qualified command id names no registered command.
    pub const UNKNOWN_COMMAND: ErrorCode = ErrorCode::from_static("plugin.unknown_command");
}
