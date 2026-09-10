//! The plugin error codes, what each one means and what fixes it.
//!
//! Every failure the host reports — building a plugin, installing it,
//! reloading it, or anything it does once it is running — arrives as one
//! JSON object with a stable shape (docs/PLAN.md §6.4):
//!
//! ```json
//! {
//!   "code": "plugin.invalid_audio_block",
//!   "message": "the returned block is not the length it was given",
//!   "details": {
//!     "wit": "subordinate:plugin/audio-effect.process",
//!     "hint": "Return exactly `frames * channels` interleaved samples."
//!   }
//! }
//! ```
//!
//! `code` is what a plugin or an agent matches on: it is part of the public
//! contract, so an existing code is never renamed or given a new meaning.
//! `message` is one human line. `details.wit` names the WIT type or function
//! the failure belongs to, and is absent for a failure that happens before
//! any WIT is involved — a `plugin.toml` that does not parse, a directory
//! that cannot be written. `details.hint` is one line saying what would fix
//! it; the rest of `details` is whatever the failure had to say: the
//! offending manifest field, the capability that was denied, the wasm
//! backtrace of a trap.
//!
//! [`CATALOGUE`] is the whole table and [`info`] looks one code up. The
//! codes group like this:
//!
//! - **The manifest**: [`INVALID_PLUGIN_ID`](codes::INVALID_PLUGIN_ID),
//!   [`INVALID_MANIFEST_SYNTAX`](codes::INVALID_MANIFEST_SYNTAX),
//!   [`INVALID_MANIFEST`](codes::INVALID_MANIFEST),
//!   [`MANIFEST_UNREADABLE`](codes::MANIFEST_UNREADABLE).
//! - **Capabilities and approval**:
//!   [`INVALID_CAPABILITY_PATH`](codes::INVALID_CAPABILITY_PATH),
//!   [`UNKNOWN_PATH_VARIABLE`](codes::UNKNOWN_PATH_VARIABLE),
//!   [`UNSET_PATH_VARIABLE`](codes::UNSET_PATH_VARIABLE),
//!   [`CAPABILITY_DENIED`](codes::CAPABILITY_DENIED),
//!   [`PREOPEN_FAILED`](codes::PREOPEN_FAILED),
//!   [`NOT_APPROVED`](codes::NOT_APPROVED),
//!   [`APPROVAL_STALE`](codes::APPROVAL_STALE),
//!   [`INVALID_APPROVALS`](codes::INVALID_APPROVALS),
//!   [`APPROVALS_UNREADABLE`](codes::APPROVALS_UNREADABLE),
//!   [`APPROVALS_UNWRITABLE`](codes::APPROVALS_UNWRITABLE),
//!   [`INVALID_DIGEST`](codes::INVALID_DIGEST).
//! - **Installing, listing and reloading**:
//!   [`DEV_SOURCE_INVALID`](codes::DEV_SOURCE_INVALID),
//!   [`INSTALL_FAILED`](codes::INSTALL_FAILED),
//!   [`INSTALL_CONFLICT`](codes::INSTALL_CONFLICT),
//!   [`NOT_INSTALLED`](codes::NOT_INSTALLED),
//!   [`PLUGIN_DIR_UNREADABLE`](codes::PLUGIN_DIR_UNREADABLE),
//!   [`PLUGIN_DIR_UNAVAILABLE`](codes::PLUGIN_DIR_UNAVAILABLE),
//!   [`INVALID_REGISTRY_STATE`](codes::INVALID_REGISTRY_STATE),
//!   [`REGISTRY_STATE_UNREADABLE`](codes::REGISTRY_STATE_UNREADABLE),
//!   [`REGISTRY_STATE_UNWRITABLE`](codes::REGISTRY_STATE_UNWRITABLE),
//!   [`REMOVE_FAILED`](codes::REMOVE_FAILED).
//! - **Building and loading a component**:
//!   [`ENGINE_FAILED`](codes::ENGINE_FAILED),
//!   [`LOAD_FAILED`](codes::LOAD_FAILED),
//!   [`LINK_FAILED`](codes::LINK_FAILED),
//!   [`INSTANTIATE_FAILED`](codes::INSTANTIATE_FAILED).
//! - **Runtime limits**: [`FUEL_EXHAUSTED`](codes::FUEL_EXHAUSTED),
//!   [`DEADLINE_EXCEEDED`](codes::DEADLINE_EXCEEDED),
//!   [`TRAPPED`](codes::TRAPPED).
//! - **What a plugin sends back across the WIT boundary**:
//!   [`INVALID_ERROR_CODE`](codes::INVALID_ERROR_CODE),
//!   [`INVALID_RATIONAL`](codes::INVALID_RATIONAL),
//!   [`INVALID_TIME_RANGE`](codes::INVALID_TIME_RANGE),
//!   [`INVALID_COMMAND_ID`](codes::INVALID_COMMAND_ID),
//!   [`INVALID_COMMAND_TITLE`](codes::INVALID_COMMAND_TITLE),
//!   [`INVALID_SHORTCUT`](codes::INVALID_SHORTCUT),
//!   [`DUPLICATE_COMMAND`](codes::DUPLICATE_COMMAND),
//!   [`UNKNOWN_COMMAND`](codes::UNKNOWN_COMMAND),
//!   [`INVALID_TOOL_NAME`](codes::INVALID_TOOL_NAME),
//!   [`INVALID_TOOL_SCHEMA`](codes::INVALID_TOOL_SCHEMA),
//!   [`DUPLICATE_TOOL`](codes::DUPLICATE_TOOL),
//!   [`UNDECLARED_TOOL`](codes::UNDECLARED_TOOL),
//!   [`INVALID_EFFECT_DECLARATION`](codes::INVALID_EFFECT_DECLARATION),
//!   [`MISSING_TOOL`](codes::MISSING_TOOL),
//!   [`TOOL_SCHEMA_MISMATCH`](codes::TOOL_SCHEMA_MISMATCH),
//!   [`INVALID_TOOL_ARGUMENTS`](codes::INVALID_TOOL_ARGUMENTS),
//!   [`INVALID_BLOCK_FORMAT`](codes::INVALID_BLOCK_FORMAT),
//!   [`INVALID_AUDIO_BLOCK`](codes::INVALID_AUDIO_BLOCK),
//!   [`INVALID_BUDGET`](codes::INVALID_BUDGET),
//!   [`AUDIO_BYPASSED`](codes::AUDIO_BYPASSED),
//!   [`INVALID_SPEC`](codes::INVALID_SPEC),
//!   [`INVALID_PRESET`](codes::INVALID_PRESET),
//!   [`INVALID_METADATA`](codes::INVALID_METADATA).
//!
//! ```
//! use subordinate_sdk::errors::{codes, info};
//!
//! let manifest = info(codes::INVALID_MANIFEST).expect("a catalogued code");
//! assert!(manifest.wit.is_none());
//! assert!(!manifest.hint.is_empty());
//! ```

/// One row of the catalogue: a code, the WIT item it belongs to and its
/// hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorInfo {
    /// The stable code, e.g. `plugin.load_failed`.
    pub code: &'static str,
    /// The WIT type or function, written as
    /// `subordinate:plugin/<interface-or-world>.<item>`, or `None` for a
    /// failure that happens outside the component interface.
    pub wit: Option<&'static str>,
    /// One line saying what would fix it.
    pub hint: &'static str,
}

/// The code constants, so a plugin matches on a name rather than on a
/// string literal.
pub mod codes {
    /// A plugin returned an error whose `code` is not a valid
    /// `ErrorCode`. The original text is kept in the
    /// `plugin_code` detail.
    pub const INVALID_ERROR_CODE: &str = "plugin.invalid_error_code";

    /// A rate crossing in from a plugin has a zero numerator or denominator.
    pub const INVALID_RATIONAL: &str = "plugin.invalid_rational";

    /// A time range crossing in from a plugin has a negative duration or two
    /// endpoints at different rates.
    pub const INVALID_TIME_RANGE: &str = "plugin.invalid_time_range";

    /// A plugin id is not a valid identifier: lowercase dot-separated
    /// segments when registering commands, two or more reverse-DNS segments
    /// when declaring MCP tools.
    pub const INVALID_PLUGIN_ID: &str = "plugin.invalid_plugin_id";

    /// A registered command's id is not a lowercase dot-separated identifier.
    pub const INVALID_COMMAND_ID: &str = "plugin.invalid_command_id";

    /// A registered command's title is empty or spans more than one line.
    pub const INVALID_COMMAND_TITLE: &str = "plugin.invalid_command_title";

    /// A registered command asks for a shortcut that is empty. An unset
    /// shortcut is how a command says it wants none.
    pub const INVALID_SHORTCUT: &str = "plugin.invalid_shortcut";

    /// One plugin registered the same command id twice.
    pub const DUPLICATE_COMMAND: &str = "plugin.duplicate_command";

    /// A qualified command id names no registered command.
    pub const UNKNOWN_COMMAND: &str = "plugin.unknown_command";

    /// An MCP tool name is not a lowercase `[a-z][a-z0-9_]*` identifier.
    pub const INVALID_TOOL_NAME: &str = "plugin.invalid_tool_name";

    /// An MCP tool's argument schema is not JSON, not an object, or not a
    /// JSON Schema the host can compile.
    pub const INVALID_TOOL_SCHEMA: &str = "plugin.invalid_tool_schema";

    /// One plugin declares the same MCP tool name twice.
    pub const DUPLICATE_TOOL: &str = "plugin.duplicate_tool";

    /// A plugin offers or is asked for an MCP tool its manifest does not
    /// declare.
    pub const UNDECLARED_TOOL: &str = "plugin.undeclared_tool";

    /// An `effect` plugin's declaration cannot be bound and the render
    /// layer's own code for it could not be parsed. The render codes —
    /// `render.invalid_effect_param` and `render.invalid_effect_shader` —
    /// are what a caller normally sees.
    pub const INVALID_EFFECT_DECLARATION: &str = "plugin.invalid_effect_declaration";

    /// A plugin's manifest declares an MCP tool the component does not export.
    pub const MISSING_TOOL: &str = "plugin.missing_tool";

    /// A plugin's exported tool schema differs from the declared one.
    pub const TOOL_SCHEMA_MISMATCH: &str = "plugin.tool_schema_mismatch";

    /// A tool call's arguments are not a JSON object, or the tool's schema
    /// rejects them.
    pub const INVALID_TOOL_ARGUMENTS: &str = "plugin.invalid_tool_arguments";

    /// A `plugin.toml` is not the TOML the manifest shape expects: a syntax
    /// error, a missing key, an unknown key or a value of the wrong type.
    pub const INVALID_MANIFEST_SYNTAX: &str = "plugin.invalid_manifest_syntax";

    /// A `plugin.toml` parses but breaks a manifest rule. The `field` detail
    /// is the dotted path of the first offending key and `fields` lists them
    /// all.
    pub const INVALID_MANIFEST: &str = "plugin.invalid_manifest";

    /// A `plugin.toml` could not be read from disk.
    pub const MANIFEST_UNREADABLE: &str = "plugin.manifest_unreadable";

    /// A capability root is not written against `$PROJECT` or `$PLUGIN_DATA`,
    /// or one of its segments is empty, relative or carries a NUL.
    pub const INVALID_CAPABILITY_PATH: &str = "plugin.invalid_capability_path";

    /// A capability root names a path variable the host does not define.
    pub const UNKNOWN_PATH_VARIABLE: &str = "plugin.unknown_path_variable";

    /// A capability root names `$PROJECT` while no project is open.
    pub const UNSET_PATH_VARIABLE: &str = "plugin.unset_path_variable";

    /// A plugin reached for a path, the network or shader compilation it was
    /// not granted. The `capability` detail names which.
    pub const CAPABILITY_DENIED: &str = "plugin.capability_denied";

    /// A granted directory could not be opened for the sandbox.
    pub const PREOPEN_FAILED: &str = "plugin.preopen_failed";

    /// A plugin was loaded with no recorded install-time approval.
    pub const NOT_APPROVED: &str = "plugin.not_approved";

    /// A plugin's manifest changed since the user approved it, so the recorded
    /// grant no longer applies and it must be approved again.
    pub const APPROVAL_STALE: &str = "plugin.approval_stale";

    /// The recorded approvals are not JSON this host can read.
    pub const INVALID_APPROVALS: &str = "plugin.invalid_approvals";

    /// The recorded approvals could not be read from disk.
    pub const APPROVALS_UNREADABLE: &str = "plugin.approvals_unreadable";

    /// The approvals could not be written to disk.
    pub const APPROVALS_UNWRITABLE: &str = "plugin.approvals_unwritable";

    /// A manifest digest is not 64 hex digits.
    pub const INVALID_DIGEST: &str = "plugin.invalid_digest";

    /// An audio block format has a zero frame count, channel count or sample
    /// rate.
    pub const INVALID_BLOCK_FORMAT: &str = "plugin.invalid_block_format";

    /// An interleaved audio buffer is not `frames * channels` samples long,
    /// either on the way into a plugin or on the way back out.
    pub const INVALID_AUDIO_BLOCK: &str = "plugin.invalid_audio_block";

    /// A real-time budget is zero, or is not between 1 and 100 percent of one
    /// block's wall-clock duration.
    pub const INVALID_BUDGET: &str = "plugin.invalid_budget";

    /// An audio effect was bypassed after repeatedly missing its real-time
    /// budget or failing outright.
    pub const AUDIO_BYPASSED: &str = "plugin.audio_bypassed";

    /// An importer described something the project model cannot hold: a media
    /// reference pointing past the end of the import, a negative gap, or a
    /// clip playing existing media without a name.
    pub const INVALID_SPEC: &str = "plugin.invalid_spec";

    /// An exporter offered a preset with a malformed id, name, container or
    /// settings, or two presets sharing an id.
    pub const INVALID_PRESET: &str = "plugin.invalid_preset";

    /// An analysis metadata value is not JSON, or a key was reported twice.
    pub const INVALID_METADATA: &str = "plugin.invalid_metadata";

    /// The wasm engine itself could not be built or configured: a build with
    /// no compiler, or fuel asked of an engine that does not meter it.
    pub const ENGINE_FAILED: &str = "plugin.engine_failed";

    /// A plugin's `.wasm` could not be read or is not a component this engine
    /// can compile.
    pub const LOAD_FAILED: &str = "plugin.load_failed";

    /// A plugin's imports could not be satisfied by the host's linker.
    pub const LINK_FAILED: &str = "plugin.link_failed";

    /// A component compiled and linked but could not be instantiated, which
    /// includes an instance whose memory ceiling is smaller than the memory it
    /// declares.
    pub const INSTANTIATE_FAILED: &str = "plugin.instantiate_failed";

    /// A plugin call ran out of its instruction budget and was stopped.
    pub const FUEL_EXHAUSTED: &str = "plugin.fuel_exhausted";

    /// A plugin call overran its wall-clock deadline and was stopped.
    pub const DEADLINE_EXCEEDED: &str = "plugin.deadline_exceeded";

    /// A plugin trapped: unreachable code, an allocation its memory ceiling
    /// refused, an out-of-bounds access.
    pub const TRAPPED: &str = "plugin.trapped";

    /// A plugin id names nothing installed in either plugin directory.
    pub const NOT_INSTALLED: &str = "plugin.not_installed";

    /// A plugin directory exists but could not be listed.
    pub const PLUGIN_DIR_UNREADABLE: &str = "plugin.plugin_dir_unreadable";

    /// This platform offers no per-user data directory to install plugins in.
    pub const PLUGIN_DIR_UNAVAILABLE: &str = "plugin.plugin_dir_unavailable";

    /// The recorded enable/disable state is not JSON this build reads, or was
    /// written by another schema version.
    pub const INVALID_REGISTRY_STATE: &str = "plugin.invalid_registry_state";

    /// The recorded enable/disable state could not be read from disk.
    pub const REGISTRY_STATE_UNREADABLE: &str = "plugin.registry_state_unreadable";

    /// The enable/disable state could not be written to disk.
    pub const REGISTRY_STATE_UNWRITABLE: &str = "plugin.registry_state_unwritable";

    /// An installed plugin's directory could not be deleted.
    pub const REMOVE_FAILED: &str = "plugin.remove_failed";

    /// What `plugin install` was pointed at is not a plugin: it does not
    /// exist, no `plugin.toml` was found beside the component or above it, or
    /// the directory holds no single `.wasm` to install.
    pub const DEV_SOURCE_INVALID: &str = "plugin.dev_source_invalid";

    /// A plugin's files could not be written into the plugin directory.
    pub const INSTALL_FAILED: &str = "plugin.install_failed";

    /// The directory a plugin would be installed into already holds a
    /// different plugin.
    pub const INSTALL_CONFLICT: &str = "plugin.install_conflict";
}

/// Every code the host raises, in one table.
pub static CATALOGUE: &[ErrorInfo] = &[
    ErrorInfo {
        code: codes::INVALID_ERROR_CODE,
        wit: Some("subordinate:plugin/types.error"),
        hint: "Return a code of two or more dot-separated lowercase segments, e.g. \
               `com.example.no_such_take`; what you sent is kept in the `plugin_code` detail.",
    },
    ErrorInfo {
        code: codes::INVALID_RATIONAL,
        wit: Some("subordinate:plugin/types.rational"),
        hint: "Send a rate with a non-zero numerator and denominator, e.g. 24000/1001, never one \
               derived from seconds.",
    },
    ErrorInfo {
        code: codes::INVALID_TIME_RANGE,
        wit: Some("subordinate:plugin/types.time-range"),
        hint: "Give the range a non-negative duration and put both endpoints on the same rate.",
    },
    ErrorInfo {
        code: codes::INVALID_PLUGIN_ID,
        wit: None,
        hint: "Use two or more dot-separated lowercase segments, e.g. `com.example.tools`, and \
               keep `plugin.id` equal to the plugin's directory name.",
    },
    ErrorInfo {
        code: codes::INVALID_COMMAND_ID,
        wit: Some("subordinate:plugin/command-menu.command-desc"),
        hint: "Give each command an id of lowercase dot-separated segments, e.g. `trim.tighten`; \
               the host qualifies it with your plugin id.",
    },
    ErrorInfo {
        code: codes::INVALID_COMMAND_TITLE,
        wit: Some("subordinate:plugin/command-menu.command-desc"),
        hint: "Give the command a non-empty single-line title: it is the menu entry a user reads.",
    },
    ErrorInfo {
        code: codes::INVALID_SHORTCUT,
        wit: Some("subordinate:plugin/command-menu.command-desc"),
        hint: "Leave `shortcut` unset to ask for no shortcut, rather than sending an empty string.",
    },
    ErrorInfo {
        code: codes::DUPLICATE_COMMAND,
        wit: Some("subordinate:plugin/commands.commands"),
        hint: "Return each command id once from `commands()`.",
    },
    ErrorInfo {
        code: codes::UNKNOWN_COMMAND,
        wit: Some("subordinate:plugin/commands.run"),
        hint: "Run a qualified id the Plugins menu lists; a reload may have withdrawn it.",
    },
    ErrorInfo {
        code: codes::INVALID_TOOL_NAME,
        wit: Some("subordinate:plugin/mcp.tool-desc"),
        hint: "Name the tool `[a-z][a-z0-9_]*`; the host prefixes your plugin id when it \
               publishes it.",
    },
    ErrorInfo {
        code: codes::INVALID_TOOL_SCHEMA,
        wit: Some("subordinate:plugin/mcp.tool-desc"),
        hint: "Send `args-schema` as a JSON object that is a JSON Schema the host can compile.",
    },
    ErrorInfo {
        code: codes::DUPLICATE_TOOL,
        wit: Some("subordinate:plugin/mcp-tools.tools"),
        hint: "Declare each tool name once, both in plugin.toml and in `tools()`.",
    },
    ErrorInfo {
        code: codes::UNDECLARED_TOOL,
        wit: Some("subordinate:plugin/mcp-tools.tools"),
        hint: "Add the tool to `[[mcp.tools]]` in plugin.toml: only declared tools are published \
               or callable.",
    },
    ErrorInfo {
        code: codes::INVALID_EFFECT_DECLARATION,
        wit: Some("subordinate:plugin/effect.describe"),
        hint: "Read `render.invalid_effect_param` and `render.invalid_effect_shader` first: they \
               name the parameter or the WGSL that could not be bound.",
    },
    ErrorInfo {
        code: codes::MISSING_TOOL,
        wit: Some("subordinate:plugin/mcp-tools.tools"),
        hint: "Export every tool plugin.toml declares, or drop it from `[[mcp.tools]]`.",
    },
    ErrorInfo {
        code: codes::TOOL_SCHEMA_MISMATCH,
        wit: Some("subordinate:plugin/mcp.tool-desc"),
        hint: "Build the exported schema and the manifest's from one source so they cannot drift.",
    },
    ErrorInfo {
        code: codes::INVALID_TOOL_ARGUMENTS,
        wit: Some("subordinate:plugin/mcp-tools.call"),
        hint: "Pass a JSON object that validates against the tool's declared schema; the failing \
               path is in the details.",
    },
    ErrorInfo {
        code: codes::INVALID_MANIFEST_SYNTAX,
        wit: None,
        hint: "Fix the TOML in plugin.toml; docs/schema/plugin-manifest.json is the shape it must \
               match.",
    },
    ErrorInfo {
        code: codes::INVALID_MANIFEST,
        wit: None,
        hint: "Correct the key named by the `field` detail; docs/schema/plugin-manifest.json is \
               the whole shape.",
    },
    ErrorInfo {
        code: codes::MANIFEST_UNREADABLE,
        wit: None,
        hint: "Check that plugin.toml exists in the plugin directory and that this user can read \
               it.",
    },
    ErrorInfo {
        code: codes::INVALID_CAPABILITY_PATH,
        wit: None,
        hint: "Write every capability root under `$PROJECT` or `$PLUGIN_DATA`, with non-empty \
               segments and no `..`.",
    },
    ErrorInfo {
        code: codes::UNKNOWN_PATH_VARIABLE,
        wit: None,
        hint: "Only `$PROJECT` and `$PLUGIN_DATA` are defined; a capability path may name no \
               other variable.",
    },
    ErrorInfo {
        code: codes::UNSET_PATH_VARIABLE,
        wit: None,
        hint: "Open a project before using a plugin whose capabilities are written against \
               `$PROJECT`.",
    },
    ErrorInfo {
        code: codes::CAPABILITY_DENIED,
        wit: None,
        hint: "Declare the capability named in the `capability` detail in plugin.toml and install \
               the plugin again so the user can approve it.",
    },
    ErrorInfo {
        code: codes::PREOPEN_FAILED,
        wit: None,
        hint: "Check that the granted directory exists and is readable: the sandbox opens it \
               before the plugin starts.",
    },
    ErrorInfo {
        code: codes::NOT_APPROVED,
        wit: None,
        hint: "Install the plugin again with `subordinate-cli plugin install` and approve the \
               capabilities it asks for.",
    },
    ErrorInfo {
        code: codes::APPROVAL_STALE,
        wit: None,
        hint: "The manifest changed since the user approved it; install the plugin again and \
               approve the capabilities it asks for now.",
    },
    ErrorInfo {
        code: codes::INVALID_APPROVALS,
        wit: None,
        hint: "The approvals file is not JSON this build reads; delete it and approve the \
               installed plugins again.",
    },
    ErrorInfo {
        code: codes::APPROVALS_UNREADABLE,
        wit: None,
        hint: "Check the permissions on the approvals file in the plugin data directory.",
    },
    ErrorInfo {
        code: codes::APPROVALS_UNWRITABLE,
        wit: None,
        hint: "Make the plugin data directory writable, then approve the plugin again.",
    },
    ErrorInfo {
        code: codes::INVALID_DIGEST,
        wit: None,
        hint: "A manifest digest is 64 lowercase hex digits; delete the approvals file if it was \
               edited by hand.",
    },
    ErrorInfo {
        code: codes::INVALID_BLOCK_FORMAT,
        wit: Some("subordinate:plugin/audio-effect.process"),
        hint: "Frames, channels and sample rate are all non-zero; the host fixes the block shape, \
               so process what you were handed rather than a shape of your own.",
    },
    ErrorInfo {
        code: codes::INVALID_AUDIO_BLOCK,
        wit: Some("subordinate:plugin/audio-effect.process"),
        hint: "Return exactly `frames * channels` interleaved samples: the same length you were \
               handed.",
    },
    ErrorInfo {
        code: codes::INVALID_BUDGET,
        wit: Some("subordinate:plugin/audio.effect-description"),
        hint: "Claim between 1 and 100 percent of one block's wall-clock duration.",
    },
    ErrorInfo {
        code: codes::AUDIO_BYPASSED,
        wit: Some("subordinate:plugin/audio-effect.process"),
        hint: "The effect missed its real-time budget or failed too often and was bypassed; make \
               `process` allocation-free and cheaper, then switch it on again.",
    },
    ErrorInfo {
        code: codes::INVALID_SPEC,
        wit: Some("subordinate:plugin/importer-api.import"),
        hint: "Keep every media reference inside the media it names, gaps non-negative, and give \
               a clip that plays existing media a name.",
    },
    ErrorInfo {
        code: codes::INVALID_PRESET,
        wit: Some("subordinate:plugin/exporter-api.presets"),
        hint: "Give each preset a unique id, a name, a container and well-formed settings.",
    },
    ErrorInfo {
        code: codes::INVALID_METADATA,
        wit: Some("subordinate:plugin/analysis.analysis-result"),
        hint: "Send each metadata value as JSON text, and each key once.",
    },
    ErrorInfo {
        code: codes::ENGINE_FAILED,
        wit: None,
        hint: "This build has no wasm compiler for the host target, or asked for fuel an engine \
               cannot meter; a normal release build runs plugins.",
    },
    ErrorInfo {
        code: codes::LOAD_FAILED,
        wit: None,
        hint: "Build the plugin for `wasm32-wasip2` as a component, then install the `.wasm` \
               again; a core module is not a component.",
    },
    ErrorInfo {
        code: codes::LINK_FAILED,
        wit: None,
        hint: "The component imports something the host does not offer: target \
               `subordinate:plugin@0.1.0` and only the worlds plugin.toml declares.",
    },
    ErrorInfo {
        code: codes::INSTANTIATE_FAILED,
        wit: None,
        hint: "The component links but will not start; check that the memory it declares fits \
               under the host's ceiling.",
    },
    ErrorInfo {
        code: codes::FUEL_EXHAUSTED,
        wit: None,
        hint: "The call ran out of its instruction budget: do less work per call, or move long \
               work into an analyzer job that reports progress.",
    },
    ErrorInfo {
        code: codes::DEADLINE_EXCEEDED,
        wit: None,
        hint: "The call overran its wall-clock deadline: return promptly and never block inside a \
               host call.",
    },
    ErrorInfo {
        code: codes::TRAPPED,
        wit: None,
        hint: "The plugin trapped on unreachable code, a refused allocation or an out-of-bounds \
               access; the `trap` detail carries the wasm backtrace.",
    },
    ErrorInfo {
        code: codes::NOT_INSTALLED,
        wit: None,
        hint: "`subordinate-cli plugin list` shows what is installed and where; check the id or \
               install it first.",
    },
    ErrorInfo {
        code: codes::PLUGIN_DIR_UNREADABLE,
        wit: None,
        hint: "Check the permissions on the plugin directory named in the details.",
    },
    ErrorInfo {
        code: codes::PLUGIN_DIR_UNAVAILABLE,
        wit: None,
        hint: "This platform offers no per-user data directory: pass `--dir` to name a plugin \
               directory explicitly.",
    },
    ErrorInfo {
        code: codes::INVALID_REGISTRY_STATE,
        wit: None,
        hint: "The plugins.json in the plugin directory was written by another schema version; \
               delete it to start again with every plugin enabled.",
    },
    ErrorInfo {
        code: codes::REGISTRY_STATE_UNREADABLE,
        wit: None,
        hint: "Check the permissions on plugins.json in the plugin directory.",
    },
    ErrorInfo {
        code: codes::REGISTRY_STATE_UNWRITABLE,
        wit: None,
        hint: "Make the plugin directory writable so enabling and disabling can be recorded.",
    },
    ErrorInfo {
        code: codes::REMOVE_FAILED,
        wit: None,
        hint: "Close whatever is holding the plugin's files open, then remove it again.",
    },
    ErrorInfo {
        code: codes::DEV_SOURCE_INVALID,
        wit: None,
        hint: "Point `plugin install` at a built `.wasm` with plugin.toml beside or above it, or \
               at the plugin directory itself.",
    },
    ErrorInfo {
        code: codes::INSTALL_FAILED,
        wit: None,
        hint: "Check that the plugin directory is writable and has room for the component.",
    },
    ErrorInfo {
        code: codes::INSTALL_CONFLICT,
        wit: None,
        hint: "Another plugin already occupies that directory: remove it first, or give this one \
               its own id.",
    },
];

/// The catalogue row for `code`.
#[must_use]
pub fn info(code: &str) -> Option<&'static ErrorInfo> {
    CATALOGUE.iter().find(|row| row.code == code)
}

#[cfg(test)]
mod tests {
    use super::{CATALOGUE, codes, info};

    #[test]
    fn every_row_is_unique_and_carries_a_hint() {
        for row in CATALOGUE {
            assert_eq!(
                CATALOGUE
                    .iter()
                    .filter(|other| other.code == row.code)
                    .count(),
                1,
                "{} appears twice",
                row.code
            );
            assert!(row.code.starts_with("plugin."), "{}", row.code);
            assert!(!row.hint.is_empty(), "{} has no hint", row.code);
        }
    }

    #[test]
    fn manifest_capability_and_reload_failures_have_distinct_codes() {
        let distinct = [
            codes::INVALID_MANIFEST,
            codes::INVALID_MANIFEST_SYNTAX,
            codes::MANIFEST_UNREADABLE,
            codes::CAPABILITY_DENIED,
            codes::NOT_APPROVED,
            codes::APPROVAL_STALE,
            codes::LOAD_FAILED,
            codes::LINK_FAILED,
            codes::INSTANTIATE_FAILED,
        ];
        for (index, code) in distinct.iter().enumerate() {
            assert!(info(code).is_some(), "{code} is not catalogued");
            assert!(
                !distinct[index + 1..].contains(code),
                "{code} is used for two situations"
            );
        }
    }

    #[test]
    fn a_wit_item_names_an_interface_or_world_and_an_item() {
        for row in CATALOGUE {
            if let Some(wit) = row.wit {
                assert!(wit.starts_with("subordinate:plugin/"), "{wit}");
                assert!(wit.contains('.'), "{wit}");
            }
        }
    }
}
