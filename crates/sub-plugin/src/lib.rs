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
//! # The effect worlds
//!
//! A WASM guest cannot touch the GPU and per-pixel loops in WASM are far too
//! slow at picture size, so a GPU effect is *declared*, not executed, by its
//! plugin (decision-6). The `effect` world exports one function,
//! `describe() -> effect-desc`, which returns a parameter schema plus WGSL
//! source and the name of its fragment entry point; the core compiles the
//! shader, caches it by hash, builds the uniform struct from the declared
//! parameters and runs it in the compositor (TASK-87). `describe` is called at
//! load time and after a hot reload, never per frame, and per-clip parameter
//! values live in the project model where ordinary undoable commands edit them.
//!
//! A parameter is a float, an int, a bool, a colour or one of a closed set of
//! choices, each carrying its range and its default: see [`WitParamKind`].
//!
//! The `effect-cpu` world is `effect` plus an optional `process-cpu` export.
//! **It is slow**: a whole frame is copied into the sandbox, looped over in
//! WASM and copied back, so it is for small buffers only — thumbnails,
//! analysis passes, test fixtures — and never for playback or export at
//! picture size. A plugin that only ships a shader targets `effect` and
//! exports nothing else.
//!
//! [`bindings`] holds the generated host side of those worlds; [`convert`] maps
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

pub use bindings::Command;
pub use bindings::effect::Effect;
pub use bindings::effect::subordinate::plugin::effect_types;
pub use bindings::effect::subordinate::plugin::effect_types::{
    BoolParam as WitBoolParam, Color as WitColor, ColorParam as WitColorParam,
    EffectDesc as WitEffectDesc, EnumParam as WitEnumParam, EnumVariant as WitEnumVariant,
    FloatParam as WitFloatParam, Frame as WitFrame, IntParam as WitIntParam,
    ParamBinding as WitParamBinding, ParamDesc as WitParamDesc, ParamKind as WitParamKind,
    ParamValue as WitParamValue, PixelFormat as WitPixelFormat,
};
pub use bindings::effect_cpu::EffectCpu;
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
}
