//! The plugin error catalogue: what every `plugin.*` code means, which WIT
//! item it belongs to, and what to do about it.
//!
//! Errors are the agent's main feedback signal (docs/PLAN.md §6.4), so a
//! plugin failure is never a log line: it is a [`SubError`] with a stable
//! code, a one-line message and two details this module adds —
//!
//! - [`WIT`] (`wit`): the WIT type or function the failure belongs to, written
//!   as `subordinate:plugin/<interface-or-world>.<item>`, e.g.
//!   `subordinate:plugin/audio-effect.process`. Absent for the failures that
//!   happen before any WIT is involved — a `plugin.toml` that does not parse,
//!   a directory that cannot be written.
//! - [`HINT`] (`hint`): one line saying what would fix it.
//!
//! [`CATALOGUE`] is the table both come from, one row per code in
//! [`crate::codes`]. [`explain`] applies it, and everything that reports a
//! plugin failure runs its errors through it: reading a `plugin.toml`,
//! scanning, enabling, disabling and removing a plugin, installing and
//! reloading one, the [`LoadFailure`](crate::LoadFailure) and
//! [`ReloadError`](crate::ReloadError) rows a scan and a reload publish, the
//! `error` record a plugin sees ([`crate::WitError`]) and the CLI's `plugin`
//! subcommands. The `plugin.*` Command API methods and the MCP bridge inherit
//! it from those. A call site that knows a more precise WIT item than the code
//! implies says so with [`PluginErrorExt::at_wit`], which [`explain`] never
//! overwrites.
//!
//! The same table is published to plugin authors as `subordinate_sdk::errors`,
//! which is where a plugin (or the agent writing one) reads the codes; the
//! `tests/errors.rs` parity test keeps the two in step.
//!
//! ```
//! use sub_plugin::codes;
//! use sub_plugin::errors::{self, HINT, PluginErrorExt, WIT};
//! use sub_core::SubError;
//!
//! let error = errors::explain(SubError::new(
//!     codes::INVALID_AUDIO_BLOCK,
//!     "the returned block is not the length it was given",
//! ));
//! assert_eq!(error.details[WIT], "subordinate:plugin/audio-effect.process");
//! assert!(error.details[HINT].as_str().unwrap().contains("interleaved"));
//!
//! // A call site may name the WIT item itself; the catalogue defers to it.
//! let error = SubError::new(codes::LOAD_FAILED, "not a component")
//!     .at_wit("subordinate:plugin/effect.describe")
//!     .explained();
//! assert_eq!(error.details[WIT], "subordinate:plugin/effect.describe");
//! ```

use sub_core::{ErrorCode, SubError, SubResult};

/// The detail key holding the WIT type or function a failure belongs to.
pub const WIT: &str = "wit";

/// The detail key holding the one-line hint.
pub const HINT: &str = "hint";

/// One row of the catalogue: a code, the WIT item it belongs to and its hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorInfo {
    /// The stable code, e.g. `plugin.load_failed`.
    pub code: &'static str,
    /// The WIT type or function, or `None` for a failure that happens outside
    /// the component interface.
    pub wit: Option<&'static str>,
    /// One line saying what would fix it.
    pub hint: &'static str,
}

/// Every `plugin.*` code this host raises, in the order [`crate::codes`]
/// declares them.
pub static CATALOGUE: &[ErrorInfo] = &[
    ErrorInfo {
        code: "plugin.invalid_error_code",
        wit: Some("subordinate:plugin/types.error"),
        hint: "Return a code of two or more dot-separated lowercase segments, e.g. \
               `com.example.no_such_take`; what you sent is kept in the `plugin_code` detail.",
    },
    ErrorInfo {
        code: "plugin.invalid_rational",
        wit: Some("subordinate:plugin/types.rational"),
        hint: "Send a rate with a non-zero numerator and denominator, e.g. 24000/1001, never one \
               derived from seconds.",
    },
    ErrorInfo {
        code: "plugin.invalid_time_range",
        wit: Some("subordinate:plugin/types.time-range"),
        hint: "Give the range a non-negative duration and put both endpoints on the same rate.",
    },
    ErrorInfo {
        code: "plugin.invalid_plugin_id",
        wit: None,
        hint: "Use two or more dot-separated lowercase segments, e.g. `com.example.tools`, and \
               keep `plugin.id` equal to the plugin's directory name.",
    },
    ErrorInfo {
        code: "plugin.invalid_command_id",
        wit: Some("subordinate:plugin/command-menu.command-desc"),
        hint: "Give each command an id of lowercase dot-separated segments, e.g. `trim.tighten`; \
               the host qualifies it with your plugin id.",
    },
    ErrorInfo {
        code: "plugin.invalid_command_title",
        wit: Some("subordinate:plugin/command-menu.command-desc"),
        hint: "Give the command a non-empty single-line title: it is the menu entry a user reads.",
    },
    ErrorInfo {
        code: "plugin.invalid_shortcut",
        wit: Some("subordinate:plugin/command-menu.command-desc"),
        hint: "Leave `shortcut` unset to ask for no shortcut, rather than sending an empty string.",
    },
    ErrorInfo {
        code: "plugin.duplicate_command",
        wit: Some("subordinate:plugin/commands.commands"),
        hint: "Return each command id once from `commands()`.",
    },
    ErrorInfo {
        code: "plugin.unknown_command",
        wit: Some("subordinate:plugin/commands.run"),
        hint: "Run a qualified id the Plugins menu lists; a reload may have withdrawn it.",
    },
    ErrorInfo {
        code: "plugin.invalid_tool_name",
        wit: Some("subordinate:plugin/mcp.tool-desc"),
        hint: "Name the tool `[a-z][a-z0-9_]*`; the host prefixes your plugin id when it \
               publishes it.",
    },
    ErrorInfo {
        code: "plugin.invalid_tool_schema",
        wit: Some("subordinate:plugin/mcp.tool-desc"),
        hint: "Send `args-schema` as a JSON object that is a JSON Schema the host can compile.",
    },
    ErrorInfo {
        code: "plugin.duplicate_tool",
        wit: Some("subordinate:plugin/mcp-tools.tools"),
        hint: "Declare each tool name once, both in plugin.toml and in `tools()`.",
    },
    ErrorInfo {
        code: "plugin.undeclared_tool",
        wit: Some("subordinate:plugin/mcp-tools.tools"),
        hint: "Add the tool to `[[mcp.tools]]` in plugin.toml: only declared tools are published \
               or callable.",
    },
    ErrorInfo {
        code: "plugin.invalid_effect_declaration",
        wit: Some("subordinate:plugin/effect.describe"),
        hint: "Read `render.invalid_effect_param` and `render.invalid_effect_shader` first: they \
               name the parameter or the WGSL that could not be bound.",
    },
    ErrorInfo {
        code: "plugin.missing_tool",
        wit: Some("subordinate:plugin/mcp-tools.tools"),
        hint: "Export every tool plugin.toml declares, or drop it from `[[mcp.tools]]`.",
    },
    ErrorInfo {
        code: "plugin.tool_schema_mismatch",
        wit: Some("subordinate:plugin/mcp.tool-desc"),
        hint: "Build the exported schema and the manifest's from one source so they cannot drift.",
    },
    ErrorInfo {
        code: "plugin.invalid_tool_arguments",
        wit: Some("subordinate:plugin/mcp-tools.call"),
        hint: "Pass a JSON object that validates against the tool's declared schema; the failing \
               path is in the details.",
    },
    ErrorInfo {
        code: "plugin.invalid_manifest_syntax",
        wit: None,
        hint: "Fix the TOML in plugin.toml; docs/schema/plugin-manifest.json is the shape it must \
               match.",
    },
    ErrorInfo {
        code: "plugin.invalid_manifest",
        wit: None,
        hint: "Correct the key named by the `field` detail; docs/schema/plugin-manifest.json is \
               the whole shape.",
    },
    ErrorInfo {
        code: "plugin.manifest_unreadable",
        wit: None,
        hint: "Check that plugin.toml exists in the plugin directory and that this user can read \
               it.",
    },
    ErrorInfo {
        code: "plugin.invalid_capability_path",
        wit: None,
        hint: "Write every capability root under `$PROJECT` or `$PLUGIN_DATA`, with non-empty \
               segments and no `..`.",
    },
    ErrorInfo {
        code: "plugin.unknown_path_variable",
        wit: None,
        hint: "Only `$PROJECT` and `$PLUGIN_DATA` are defined; a capability path may name no \
               other variable.",
    },
    ErrorInfo {
        code: "plugin.unset_path_variable",
        wit: None,
        hint: "Open a project before using a plugin whose capabilities are written against \
               `$PROJECT`.",
    },
    ErrorInfo {
        code: "plugin.capability_denied",
        wit: None,
        hint: "Declare the capability named in the `capability` detail in plugin.toml and install \
               the plugin again so the user can approve it.",
    },
    ErrorInfo {
        code: "plugin.preopen_failed",
        wit: None,
        hint: "Check that the granted directory exists and is readable: the sandbox opens it \
               before the plugin starts.",
    },
    ErrorInfo {
        code: "plugin.not_approved",
        wit: None,
        hint: "Install the plugin again with `subordinate-cli plugin install` and approve the \
               capabilities it asks for.",
    },
    ErrorInfo {
        code: "plugin.approval_stale",
        wit: None,
        hint: "The manifest changed since the user approved it; install the plugin again and \
               approve the capabilities it asks for now.",
    },
    ErrorInfo {
        code: "plugin.invalid_approvals",
        wit: None,
        hint: "The approvals file is not JSON this build reads; delete it and approve the \
               installed plugins again.",
    },
    ErrorInfo {
        code: "plugin.approvals_unreadable",
        wit: None,
        hint: "Check the permissions on the approvals file in the plugin data directory.",
    },
    ErrorInfo {
        code: "plugin.approvals_unwritable",
        wit: None,
        hint: "Make the plugin data directory writable, then approve the plugin again.",
    },
    ErrorInfo {
        code: "plugin.invalid_digest",
        wit: None,
        hint: "A manifest digest is 64 lowercase hex digits; delete the approvals file if it was \
               edited by hand.",
    },
    ErrorInfo {
        code: "plugin.invalid_block_format",
        wit: Some("subordinate:plugin/audio-effect.process"),
        hint: "Frames, channels and sample rate are all non-zero; the host fixes the block shape, \
               so process what you were handed rather than a shape of your own.",
    },
    ErrorInfo {
        code: "plugin.invalid_audio_block",
        wit: Some("subordinate:plugin/audio-effect.process"),
        hint: "Return exactly `frames * channels` interleaved samples: the same length you were \
               handed.",
    },
    ErrorInfo {
        code: "plugin.invalid_budget",
        wit: Some("subordinate:plugin/audio.effect-description"),
        hint: "Claim between 1 and 100 percent of one block's wall-clock duration.",
    },
    ErrorInfo {
        code: "plugin.audio_bypassed",
        wit: Some("subordinate:plugin/audio-effect.process"),
        hint: "The effect missed its real-time budget or failed too often and was bypassed; make \
               `process` allocation-free and cheaper, then switch it on again.",
    },
    ErrorInfo {
        code: "plugin.invalid_spec",
        wit: Some("subordinate:plugin/importer-api.import"),
        hint: "Keep every media reference inside the media it names, gaps non-negative, and give \
               a clip that plays existing media a name.",
    },
    ErrorInfo {
        code: "plugin.invalid_preset",
        wit: Some("subordinate:plugin/exporter-api.presets"),
        hint: "Give each preset a unique id, a name, a container and well-formed settings.",
    },
    ErrorInfo {
        code: "plugin.invalid_metadata",
        wit: Some("subordinate:plugin/analysis.analysis-result"),
        hint: "Send each metadata value as JSON text, and each key once.",
    },
    ErrorInfo {
        code: "plugin.engine_failed",
        wit: None,
        hint: "This build has no wasm compiler for the host target, or asked for fuel an engine \
               cannot meter; a normal release build runs plugins.",
    },
    ErrorInfo {
        code: "plugin.load_failed",
        wit: None,
        hint: "Build the plugin for `wasm32-wasip2` as a component, then install the `.wasm` \
               again; a core module is not a component.",
    },
    ErrorInfo {
        code: "plugin.link_failed",
        wit: None,
        hint: "The component imports something the host does not offer: target \
               `subordinate:plugin@0.1.0` and only the worlds plugin.toml declares.",
    },
    ErrorInfo {
        code: "plugin.instantiate_failed",
        wit: None,
        hint: "The component links but will not start; check that the memory it declares fits \
               under the host's ceiling.",
    },
    ErrorInfo {
        code: "plugin.fuel_exhausted",
        wit: None,
        hint: "The call ran out of its instruction budget: do less work per call, or move long \
               work into an analyzer job that reports progress.",
    },
    ErrorInfo {
        code: "plugin.deadline_exceeded",
        wit: None,
        hint: "The call overran its wall-clock deadline: return promptly and never block inside a \
               host call.",
    },
    ErrorInfo {
        code: "plugin.trapped",
        wit: None,
        hint: "The plugin trapped on unreachable code, a refused allocation or an out-of-bounds \
               access; the `trap` detail carries the wasm backtrace.",
    },
    ErrorInfo {
        code: "plugin.not_installed",
        wit: None,
        hint: "`subordinate-cli plugin list` shows what is installed and where; check the id or \
               install it first.",
    },
    ErrorInfo {
        code: "plugin.plugin_dir_unreadable",
        wit: None,
        hint: "Check the permissions on the plugin directory named in the details.",
    },
    ErrorInfo {
        code: "plugin.plugin_dir_unavailable",
        wit: None,
        hint: "This platform offers no per-user data directory: pass `--dir` to name a plugin \
               directory explicitly.",
    },
    ErrorInfo {
        code: "plugin.invalid_registry_state",
        wit: None,
        hint: "The plugins.json in the plugin directory was written by another schema version; \
               delete it to start again with every plugin enabled.",
    },
    ErrorInfo {
        code: "plugin.registry_state_unreadable",
        wit: None,
        hint: "Check the permissions on plugins.json in the plugin directory.",
    },
    ErrorInfo {
        code: "plugin.registry_state_unwritable",
        wit: None,
        hint: "Make the plugin directory writable so enabling and disabling can be recorded.",
    },
    ErrorInfo {
        code: "plugin.remove_failed",
        wit: None,
        hint: "Close whatever is holding the plugin's files open, then remove it again.",
    },
    ErrorInfo {
        code: "plugin.dev_source_invalid",
        wit: None,
        hint: "Point `plugin install` at a built `.wasm` with plugin.toml beside or above it, or \
               at the plugin directory itself.",
    },
    ErrorInfo {
        code: "plugin.install_failed",
        wit: None,
        hint: "Check that the plugin directory is writable and has room for the component.",
    },
    ErrorInfo {
        code: "plugin.install_conflict",
        wit: None,
        hint: "Another plugin already occupies that directory: remove it first, or give this one \
               its own id.",
    },
];

/// The catalogue row for `code`, if it is one this host owns.
#[must_use]
pub fn info(code: &ErrorCode) -> Option<&'static ErrorInfo> {
    CATALOGUE.iter().find(|row| row.code == code.as_str())
}

/// Adds the `wit` and `hint` details `error`'s code implies.
///
/// Neither detail is overwritten: a call site that named a more precise WIT
/// item keeps it, and an error whose code is not a `plugin.*` one — an I/O
/// error on the way to a plugin directory, a Command API parameter mismatch —
/// crosses unchanged.
#[must_use]
pub fn explain(error: SubError) -> SubError {
    let Some(row) = info(&error.code) else {
        return error;
    };
    let mut error = error;
    if let Some(wit) = row.wit
        && !error.details.contains_key(WIT)
    {
        error = error.with_detail(WIT, wit);
    }
    if !error.details.contains_key(HINT) {
        error = error.with_detail(HINT, row.hint);
    }
    error
}

/// Applies [`explain`] to a failed result.
///
/// # Errors
///
/// Returns the explained error when `result` is `Err`.
pub fn explained<T>(result: SubResult<T>) -> SubResult<T> {
    result.map_err(explain)
}

/// The two things a plugin failure carries beyond its code and message.
pub trait PluginErrorExt: Sized {
    /// Records the WIT type or function this failure belongs to, written as
    /// `subordinate:plugin/<interface-or-world>.<item>`.
    #[must_use]
    fn at_wit(self, wit: impl Into<String>) -> Self;

    /// Fills in whichever of `wit` and `hint` the code implies and the error
    /// does not already carry. See [`explain`].
    #[must_use]
    fn explained(self) -> Self;
}

impl PluginErrorExt for SubError {
    fn at_wit(self, wit: impl Into<String>) -> Self {
        self.with_detail(WIT, wit.into())
    }

    fn explained(self) -> Self {
        explain(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{CATALOGUE, ErrorInfo, HINT, PluginErrorExt, WIT, explain, info};
    use crate::codes;
    use sub_core::{ErrorCode, SubError};

    /// Every code [`crate::codes`] declares, so the catalogue cannot fall
    /// behind the constants. Adding a code without a row fails here.
    fn declared() -> Vec<ErrorCode> {
        vec![
            codes::INVALID_ERROR_CODE,
            codes::INVALID_RATIONAL,
            codes::INVALID_TIME_RANGE,
            codes::INVALID_PLUGIN_ID,
            codes::INVALID_COMMAND_ID,
            codes::INVALID_COMMAND_TITLE,
            codes::INVALID_SHORTCUT,
            codes::DUPLICATE_COMMAND,
            codes::UNKNOWN_COMMAND,
            codes::INVALID_TOOL_NAME,
            codes::INVALID_TOOL_SCHEMA,
            codes::DUPLICATE_TOOL,
            codes::UNDECLARED_TOOL,
            codes::INVALID_EFFECT_DECLARATION,
            codes::MISSING_TOOL,
            codes::TOOL_SCHEMA_MISMATCH,
            codes::INVALID_TOOL_ARGUMENTS,
            codes::INVALID_MANIFEST_SYNTAX,
            codes::INVALID_MANIFEST,
            codes::MANIFEST_UNREADABLE,
            codes::INVALID_CAPABILITY_PATH,
            codes::UNKNOWN_PATH_VARIABLE,
            codes::UNSET_PATH_VARIABLE,
            codes::CAPABILITY_DENIED,
            codes::PREOPEN_FAILED,
            codes::NOT_APPROVED,
            codes::APPROVAL_STALE,
            codes::INVALID_APPROVALS,
            codes::APPROVALS_UNREADABLE,
            codes::APPROVALS_UNWRITABLE,
            codes::INVALID_DIGEST,
            codes::INVALID_BLOCK_FORMAT,
            codes::INVALID_AUDIO_BLOCK,
            codes::INVALID_BUDGET,
            codes::AUDIO_BYPASSED,
            codes::INVALID_SPEC,
            codes::INVALID_PRESET,
            codes::INVALID_METADATA,
            codes::ENGINE_FAILED,
            codes::LOAD_FAILED,
            codes::LINK_FAILED,
            codes::INSTANTIATE_FAILED,
            codes::FUEL_EXHAUSTED,
            codes::DEADLINE_EXCEEDED,
            codes::TRAPPED,
            codes::NOT_INSTALLED,
            codes::PLUGIN_DIR_UNREADABLE,
            codes::PLUGIN_DIR_UNAVAILABLE,
            codes::INVALID_REGISTRY_STATE,
            codes::REGISTRY_STATE_UNREADABLE,
            codes::REGISTRY_STATE_UNWRITABLE,
            codes::REMOVE_FAILED,
            codes::DEV_SOURCE_INVALID,
            codes::INSTALL_FAILED,
            codes::INSTALL_CONFLICT,
        ]
    }

    #[test]
    fn every_declared_code_has_exactly_one_row() {
        let declared = declared();
        assert_eq!(declared.len(), CATALOGUE.len(), "the catalogue is stale");
        for code in &declared {
            let rows = CATALOGUE
                .iter()
                .filter(|row| row.code == code.as_str())
                .count();
            assert_eq!(rows, 1, "{code} has {rows} rows");
        }
    }

    #[test]
    fn the_catalogue_is_in_the_order_the_codes_are_declared() {
        let declared: Vec<String> = declared()
            .iter()
            .map(|code| code.as_str().to_owned())
            .collect();
        let catalogued: Vec<String> = CATALOGUE.iter().map(|row| row.code.to_owned()).collect();
        assert_eq!(declared, catalogued);
    }

    #[test]
    fn every_row_is_well_formed() {
        for &ErrorInfo { code, wit, hint } in CATALOGUE {
            assert!(
                ErrorCode::parse(code).is_ok_and(|parsed| parsed.domain() == "plugin"),
                "{code} is not a plugin code"
            );
            assert!(!hint.is_empty(), "{code} has no hint");
            assert!(!hint.contains('\n'), "{code}'s hint is more than one line");
            // A hint reads as an instruction: it starts with a capital, or
            // with the backtick of a command to run.
            let first = hint.chars().next().expect("a non-empty hint");
            assert!(
                first.is_uppercase() || first == '`',
                "{code}'s hint does not read as a sentence"
            );
            if let Some(wit) = wit {
                assert!(
                    wit.starts_with("subordinate:plugin/") && wit.contains('.'),
                    "{code} names {wit}, which is not a WIT item"
                );
            }
        }
    }

    #[test]
    fn explaining_adds_the_wit_item_and_the_hint() {
        let error = explain(SubError::new(codes::INVALID_AUDIO_BLOCK, "wrong length"));
        assert_eq!(
            error.details[WIT],
            "subordinate:plugin/audio-effect.process"
        );
        assert_eq!(
            error.details[HINT],
            info(&codes::INVALID_AUDIO_BLOCK).expect("a row").hint
        );
    }

    #[test]
    fn a_code_with_no_wit_item_still_gets_a_hint() {
        let error = explain(SubError::new(codes::INVALID_MANIFEST, "unknown key"));
        assert!(!error.details.contains_key(WIT));
        assert!(
            error.details[HINT]
                .as_str()
                .is_some_and(|hint| hint.contains("docs/schema/plugin-manifest.json"))
        );
    }

    #[test]
    fn a_call_site_that_named_the_wit_item_keeps_it() {
        let error = SubError::new(codes::LOAD_FAILED, "not a component")
            .at_wit("subordinate:plugin/effect.describe")
            .explained();
        assert_eq!(error.details[WIT], "subordinate:plugin/effect.describe");
    }

    #[test]
    fn explaining_is_idempotent_and_leaves_foreign_codes_alone() {
        let once = explain(SubError::new(codes::TRAPPED, "unreachable"));
        assert_eq!(explain(once.clone()), once);

        let foreign = SubError::new(sub_core::codes::IO, "disk is full");
        assert_eq!(explain(foreign.clone()), foreign);
    }
}
