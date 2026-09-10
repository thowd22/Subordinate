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
//! # The interchange worlds
//!
//! `importer` and `exporter` build on that same import (TASK-78):
//!
//! - `importer-api.supported-extensions()` and `importer-api.import(path)` —
//!   the plugin parses a file and describes the media items and sequences it
//!   found; it never edits the project. [`import_plan`] turns that description
//!   into the `media.import` and `sequence.insert` calls the host applies, so
//!   an import is undoable and every identifier is minted host-side.
//! - `exporter-api.presets()` and `exporter-api.post-export(path)` — the
//!   plugin contributes named encoder presets and may act on the file once the
//!   host has written it. [`export_presets`] validates the preset records into
//!   the [`ExportPreset`] rows the export panel (TASK-62) lists.
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
mod interchange;

pub use bindings::Command;
pub use bindings::exporter::Exporter;
pub use bindings::exporter::exports::subordinate::plugin::exporter_api::{
    PresetDesc as WitPresetDesc, PresetSetting as WitPresetSetting,
};
pub use bindings::importer::Importer;
pub use bindings::importer::exports::subordinate::plugin::importer_api::{
    ClipSpec as WitClipSpec, MarkerSpec as WitMarkerSpec, MediaOrSequenceSpec as WitImportSpec,
    MediaRef as WitMediaRef, MediaSpec as WitMediaSpec, SequenceSpec as WitSequenceSpec,
    TrackItemSpec as WitTrackItemSpec, TrackSpec as WitTrackSpec,
    TransitionSpec as WitTransitionSpec,
};
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
pub use interchange::{CommandCall, ExportPreset, ImportTarget, export_presets, import_plan};

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
    /// An importer described something the project model cannot hold: a media
    /// reference pointing past the end of the import, a negative gap, or a
    /// clip playing existing media without a name.
    pub const INVALID_SPEC: ErrorCode = ErrorCode::from_static("plugin.invalid_spec");
    /// An exporter offered a preset with a malformed id, name, container or
    /// settings, or two presets sharing an id.
    pub const INVALID_PRESET: ErrorCode = ErrorCode::from_static("plugin.invalid_preset");
}
