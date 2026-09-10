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
//! # Worlds
//!
//! `command` runs one plugin command against a project. `mcp-tools` lets a
//! plugin contribute MCP tools of its own: it exports `tools()` and
//! `call(name, args-json)`, and the host publishes each tool under the plugin's
//! id so an agent sees plugin tools beside the built-in ones. [`mcp`] is the
//! host half of that world — the manifest's declarations, the check that the
//! component's exports match them, and the argument validation a call passes
//! before it reaches the plugin.
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
pub mod mcp;

pub use bindings::Command;
pub use bindings::mcp_tools::McpTools;
pub use bindings::mcp_tools::subordinate::plugin::mcp as mcp_types;
pub use bindings::mcp_tools::subordinate::plugin::mcp::ToolDesc as WitToolDesc;
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
    /// A plugin id is not two or more lowercase reverse-DNS segments.
    pub const INVALID_PLUGIN_ID: ErrorCode = ErrorCode::from_static("plugin.invalid_plugin_id");
    /// An MCP tool name is not a lowercase `[a-z][a-z0-9_]*` identifier.
    pub const INVALID_TOOL_NAME: ErrorCode = ErrorCode::from_static("plugin.invalid_tool_name");
    /// An MCP tool's argument schema is not JSON, not an object, or not a
    /// JSON Schema the host can compile.
    pub const INVALID_TOOL_SCHEMA: ErrorCode = ErrorCode::from_static("plugin.invalid_tool_schema");
    /// One plugin declares the same MCP tool name twice.
    pub const DUPLICATE_TOOL: ErrorCode = ErrorCode::from_static("plugin.duplicate_tool");
    /// A plugin offers or is asked for an MCP tool its manifest does not
    /// declare.
    pub const UNDECLARED_TOOL: ErrorCode = ErrorCode::from_static("plugin.undeclared_tool");
    /// A plugin's manifest declares an MCP tool the component does not export.
    pub const MISSING_TOOL: ErrorCode = ErrorCode::from_static("plugin.missing_tool");
    /// A plugin's exported tool schema differs from the declared one.
    pub const TOOL_SCHEMA_MISMATCH: ErrorCode =
        ErrorCode::from_static("plugin.tool_schema_mismatch");
    /// A tool call's arguments are not a JSON object, or the tool's schema
    /// rejects them.
    pub const INVALID_TOOL_ARGUMENTS: ErrorCode =
        ErrorCode::from_static("plugin.invalid_tool_arguments");
}
