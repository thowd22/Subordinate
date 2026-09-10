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
//! # Plugin-contributed MCP tools
//!
//! The `mcp-tools` world lets a plugin contribute MCP tools of its own: it
//! exports `tools()` and `call(name, args-json)`, and the host publishes each
//! tool under the plugin's id so an agent sees plugin tools beside the built-in
//! ones. [`mcp`] is the host half of that world — the manifest's declarations,
//! the check that the component's exports match them, and the argument
//! validation a call passes before it reaches the plugin.
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
//! # The analyzer world
//!
//! `analyzer` is background analysis (docs/PLAN.md §6.2): it exports
//! `analyze(media, options)` and adds one import, `analysis-host`, the progress
//! and cancellation channel of the job the run is. [`analyzer`] is the host
//! side of it — [`AnalysisJobs`] puts a run on the shared
//! [`JobService`](sub_core::jobs::JobService), forwards progress, honours
//! cancellation, and stores what was found on the media item by applying
//! `media.set_analysis` through the engine, so findings arrive as an ordinary
//! undoable command.
//!
//! [`bindings`] holds the generated host side of those worlds; [`convert`] maps
//! its records onto [`sub_core::SubError`], [`sub_time::RationalTime`] and the
//! `sub_model` identifiers, so an implementation of the generated `Host` trait
//! (TASK-84) never hand-builds a record. `convert` is also where an analyzer's
//! findings become a [`sub_model::Analysis`], marker identifiers and all.
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
//!
//! # The runtime
//!
//! [`runtime`] is the wasmtime host itself (docs/PLAN.md §4): one
//! [`PluginRuntime`] owns the engine every plugin shares and the single ticker
//! thread its epoch deadlines are counted in, [`Limits`] carries the fuel,
//! deadline and memory ceilings one instance runs under, and [`termination`]
//! turns a stopped call into a [`SubError`](sub_core::SubError) with a stable
//! code — so a plugin that loops forever is reported, not fatal, and every
//! other plugin on the same engine keeps running. [`InstancePool`] keeps warm
//! instances for the hot paths — an effect's `describe`, a plugin command from
//! a menu — and re-arms their budgets on every checkout.
//!
//! # The manifest
//!
//! [`manifest`] parses `plugin.toml` (docs/PLAN.md §6.3): identity, the
//! interface version the plugin was built against, its WIT worlds, the
//! capabilities it asks for and the MCP tools it contributes. [`schema`]
//! exports its JSON Schema, committed at `docs/schema/plugin-manifest.json`.
//!
//! # The registry
//!
//! [`registry`] is where plugins are installed and which of them are switched
//! on (docs/PLAN.md §6.4): [`PluginDirs`] pairs the per-user plugin directory
//! with a project's own `.subordinate/plugins`, [`PluginRegistry::scan`] parses
//! every manifest it finds there — reporting a broken one as a
//! [`LoadFailure`] rather than giving up on the rest — and the project-local
//! copy wins an id conflict, with the hidden one recorded as [`Shadowed`].
//! [`registry::register_methods`] puts `plugin.list`, `plugin.enable`,
//! `plugin.disable` and `plugin.remove` on the Command API, so the CLI, the
//! GUI and an agent over MCP manage plugins through one surface.
//!
//! # The capability model
//!
//! A manifest only *asks*. [`capability`] is what the asking turns into
//! (docs/PLAN.md §6.1): [`ApprovalStore`] records what the user approved when
//! the plugin was installed and refuses to load a `plugin.toml` that has
//! changed since, and [`ResolvedCapabilities`] expands the approved roots —
//! `$PROJECT` and `$PLUGIN_DATA` — into the WASI [`Preopen`]s the sandbox opens
//! and the gates every host function checks before it reaches a file, a socket
//! or a shader compiler on a plugin's behalf. Nothing is granted by default.

pub mod analyzer;
pub mod audio;
pub mod bindings;
pub mod capability;
mod convert;
mod interchange;
pub mod manifest;
pub mod mcp;
pub mod menu;
pub mod registry;
pub mod runtime;
pub mod schema;

pub use capability::{
    Access, Approval, ApprovalStatus, ApprovalStore, ManifestDigest, PathVars, Preopen,
    ResolvedCapabilities,
};
pub use manifest::{MANIFEST_FILE_NAME, Manifest, PluginId, World};

pub use analyzer::{AnalysisContext, AnalysisJobs, Analyzer};
pub use bindings::Command;
pub use bindings::analyzer::Analyzer as AnalyzerWorld;
pub use bindings::analyzer::subordinate::plugin::analysis;
pub use bindings::analyzer::subordinate::plugin::analysis::{
    AnalysisMarker as WitAnalysisMarker, AnalysisRange as WitAnalysisRange,
    AnalysisResult as WitAnalysisResult,
};
pub use bindings::analyzer::subordinate::plugin::analysis_host;
pub use bindings::audio::AudioEffect;
pub use bindings::audio::subordinate::plugin::audio as audio_types;
pub use bindings::audio::subordinate::plugin::audio::{
    EffectDescription, Param, ParamDescriptor, ParamUnit,
};
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
pub use bindings::mcp_tools::McpTools;
pub use bindings::mcp_tools::subordinate::plugin::mcp as mcp_types;
pub use bindings::mcp_tools::subordinate::plugin::mcp::ToolDesc as WitToolDesc;
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
    analysis as analysis_from_wit, marker_metadata, project_metadata, sequence_metadata,
    track_clip_metadata, track_metadata,
};
pub use interchange::{CommandCall, ExportPreset, ImportTarget, export_presets, import_plan};
pub use menu::{
    CommandContext, CommandDesc, PluginCommand, PluginCommandRegistry, QUALIFIED_SEPARATOR,
    run_as_undo_group,
};
pub use registry::{
    EnableChange, InstallLocation, InstalledPlugin, LoadFailure, PluginDirs, PluginRegistry,
    Removal, Scan, Shadowed,
};
pub use runtime::{
    InstancePool, Limits, PluginRuntime, PluginState, Termination, WarmInstance, termination,
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
    /// A plugin id is not a valid identifier: lowercase dot-separated
    /// segments when registering commands, two or more reverse-DNS segments
    /// when declaring MCP tools.
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
    /// A `plugin.toml` is not the TOML the manifest shape expects: a syntax
    /// error, a missing key, an unknown key or a value of the wrong type.
    pub const INVALID_MANIFEST_SYNTAX: ErrorCode =
        ErrorCode::from_static("plugin.invalid_manifest_syntax");
    /// A `plugin.toml` parses but breaks a manifest rule. The `field` detail
    /// is the dotted path of the first offending key and `fields` lists them
    /// all.
    pub const INVALID_MANIFEST: ErrorCode = ErrorCode::from_static("plugin.invalid_manifest");
    /// A `plugin.toml` could not be read from disk.
    pub const MANIFEST_UNREADABLE: ErrorCode = ErrorCode::from_static("plugin.manifest_unreadable");
    /// A capability root is not written against `$PROJECT` or `$PLUGIN_DATA`,
    /// or one of its segments is empty, relative or carries a NUL.
    pub const INVALID_CAPABILITY_PATH: ErrorCode =
        ErrorCode::from_static("plugin.invalid_capability_path");
    /// A capability root names a path variable the host does not define.
    pub const UNKNOWN_PATH_VARIABLE: ErrorCode =
        ErrorCode::from_static("plugin.unknown_path_variable");
    /// A capability root names `$PROJECT` while no project is open.
    pub const UNSET_PATH_VARIABLE: ErrorCode = ErrorCode::from_static("plugin.unset_path_variable");
    /// A plugin reached for a path, the network or shader compilation it was
    /// not granted. The `capability` detail names which.
    pub const CAPABILITY_DENIED: ErrorCode = ErrorCode::from_static("plugin.capability_denied");
    /// A granted directory could not be opened for the sandbox.
    pub const PREOPEN_FAILED: ErrorCode = ErrorCode::from_static("plugin.preopen_failed");
    /// A plugin was loaded with no recorded install-time approval.
    pub const NOT_APPROVED: ErrorCode = ErrorCode::from_static("plugin.not_approved");
    /// A plugin's manifest changed since the user approved it, so the recorded
    /// grant no longer applies and it must be approved again.
    pub const APPROVAL_STALE: ErrorCode = ErrorCode::from_static("plugin.approval_stale");
    /// The recorded approvals are not JSON this host can read.
    pub const INVALID_APPROVALS: ErrorCode = ErrorCode::from_static("plugin.invalid_approvals");
    /// The recorded approvals could not be read from disk.
    pub const APPROVALS_UNREADABLE: ErrorCode =
        ErrorCode::from_static("plugin.approvals_unreadable");
    /// The approvals could not be written to disk.
    pub const APPROVALS_UNWRITABLE: ErrorCode =
        ErrorCode::from_static("plugin.approvals_unwritable");
    /// A manifest digest is not 64 hex digits.
    pub const INVALID_DIGEST: ErrorCode = ErrorCode::from_static("plugin.invalid_digest");
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
    /// An importer described something the project model cannot hold: a media
    /// reference pointing past the end of the import, a negative gap, or a
    /// clip playing existing media without a name.
    pub const INVALID_SPEC: ErrorCode = ErrorCode::from_static("plugin.invalid_spec");
    /// An exporter offered a preset with a malformed id, name, container or
    /// settings, or two presets sharing an id.
    pub const INVALID_PRESET: ErrorCode = ErrorCode::from_static("plugin.invalid_preset");
    /// An analysis metadata value is not JSON, or a key was reported twice.
    pub const INVALID_METADATA: ErrorCode = ErrorCode::from_static("plugin.invalid_metadata");
    /// The wasm engine itself could not be built or configured: a build with
    /// no compiler, or fuel asked of an engine that does not meter it.
    pub const ENGINE_FAILED: ErrorCode = ErrorCode::from_static("plugin.engine_failed");
    /// A plugin's `.wasm` could not be read or is not a component this engine
    /// can compile.
    pub const LOAD_FAILED: ErrorCode = ErrorCode::from_static("plugin.load_failed");
    /// A plugin's imports could not be satisfied by the host's linker.
    pub const LINK_FAILED: ErrorCode = ErrorCode::from_static("plugin.link_failed");
    /// A component compiled and linked but could not be instantiated, which
    /// includes an instance whose memory ceiling is smaller than the memory it
    /// declares.
    pub const INSTANTIATE_FAILED: ErrorCode = ErrorCode::from_static("plugin.instantiate_failed");
    /// A plugin call ran out of its instruction budget and was stopped.
    pub const FUEL_EXHAUSTED: ErrorCode = ErrorCode::from_static("plugin.fuel_exhausted");
    /// A plugin call overran its wall-clock deadline and was stopped.
    pub const DEADLINE_EXCEEDED: ErrorCode = ErrorCode::from_static("plugin.deadline_exceeded");
    /// A plugin trapped: unreachable code, an allocation its memory ceiling
    /// refused, an out-of-bounds access.
    pub const TRAPPED: ErrorCode = ErrorCode::from_static("plugin.trapped");
    /// A plugin id names nothing installed in either plugin directory.
    pub const NOT_INSTALLED: ErrorCode = ErrorCode::from_static("plugin.not_installed");
    /// A plugin directory exists but could not be listed.
    pub const PLUGIN_DIR_UNREADABLE: ErrorCode =
        ErrorCode::from_static("plugin.plugin_dir_unreadable");
    /// This platform offers no per-user data directory to install plugins in.
    pub const PLUGIN_DIR_UNAVAILABLE: ErrorCode =
        ErrorCode::from_static("plugin.plugin_dir_unavailable");
    /// The recorded enable/disable state is not JSON this build reads, or was
    /// written by another schema version.
    pub const INVALID_REGISTRY_STATE: ErrorCode =
        ErrorCode::from_static("plugin.invalid_registry_state");
    /// The recorded enable/disable state could not be read from disk.
    pub const REGISTRY_STATE_UNREADABLE: ErrorCode =
        ErrorCode::from_static("plugin.registry_state_unreadable");
    /// The enable/disable state could not be written to disk.
    pub const REGISTRY_STATE_UNWRITABLE: ErrorCode =
        ErrorCode::from_static("plugin.registry_state_unwritable");
    /// An installed plugin's directory could not be deleted.
    pub const REMOVE_FAILED: ErrorCode = ErrorCode::from_static("plugin.remove_failed");
}
