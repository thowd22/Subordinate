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
//! # The audio-effect world
//!
//! `audio-effect` is the second world (docs/PLAN.md §6.2). A plugin exports
//! `describe()`, which reports its identity, its parameters and the share of
//! real time it claims, and `process(block, channels, rate, params)`, which
//! maps one buffer of interleaved `f32` samples onto another of the same shape.
//!
//! The host half of that contract is [`audio`]: [`audio::BlockFormat`] fixes
//! and checks the block shape, and [`audio::RealTimeBudget`] times every block
//! against a fraction of the block's own wall-clock duration and bypasses the
//! plugin once it misses that deadline — or fails outright — too many times in
//! a row. Neither type allocates or locks. `plugins/gain` is the reference
//! plugin for the world.
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

pub mod audio;
pub mod bindings;
mod convert;

pub use bindings::Command;
pub use bindings::audio::AudioEffect;
pub use bindings::audio::subordinate::plugin::audio as audio_types;
pub use bindings::audio::subordinate::plugin::audio::{
    EffectDescription, Param, ParamDescriptor, ParamUnit,
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
    /// An audio block format has a zero frame count, channel count or sample
    /// rate.
    pub const INVALID_BLOCK_FORMAT: ErrorCode =
        ErrorCode::from_static("plugin.invalid_block_format");
    /// An interleaved audio buffer is not `frames * channels` samples long,
    /// either on the way into a plugin or on the way back out.
    pub const INVALID_AUDIO_BLOCK: ErrorCode = ErrorCode::from_static("plugin.invalid_audio_block");
    /// A real-time budget is zero, or is not between 1 and 100 percent of one
    /// block's wall-clock duration.
    pub const INVALID_BUDGET: ErrorCode = ErrorCode::from_static("plugin.invalid_budget");
    /// An audio effect was bypassed after repeatedly missing its real-time
    /// budget or failing outright.
    pub const AUDIO_BYPASSED: ErrorCode = ErrorCode::from_static("plugin.audio_bypassed");
}
