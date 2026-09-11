//! Plugin SDK for Subordinate.
//!
//! Wraps the generated WIT guest bindings with ergonomic types and helpers so a
//! plugin author — human or agent — never touches raw component boilerplate.
//! Licensed MIT OR Apache-2.0, so a plugin built on it may use any licence
//! (decision-2, docs/PLAN.md §6.4).
//!
//! # The shape of a plugin
//!
//! A plugin is one WASM component implementing one `subordinate:plugin@0.1.0`
//! world. Pick the world as a cargo feature, implement the `Guest` trait it
//! gives you, and hand that implementation to [`export!`]:
//!
//! ```toml
//! [lib]
//! crate-type = ["cdylib"]
//!
//! [dependencies]
//! subordinate-sdk = "0.1"           # the `command` world, the default
//! ```
//!
//! That one dependency is the whole list. A plugin's answers and arguments are
//! JSON, and the SDK re-exports [`serde_json`] and its [`json!`](json) macro, so
//! nothing has to be added to build one.
//!
//! ```ignore
//! use subordinate_sdk::{Guest, ProjectId, Result, export, json};
//!
//! struct Plugin;
//!
//! impl Guest for Plugin {
//!     fn run(project: ProjectId, args: String) -> Result<String> {
//!         let project = subordinate_sdk::Project::new(project);
//!         let revision = project.query(&subordinate_sdk::params::ProjectRevision {})?;
//!         Ok(json!({ "revision": revision.revision }).to_string())
//!     }
//! }
//!
//! export!(Plugin);
//! ```
//!
//! Build it with `cargo build --release --target wasm32-wasip2`; the Rust
//! toolchain emits a component directly, so there is no `cargo component` or
//! `wasm-tools` step.
//!
//! # Editing goes through the Command API
//!
//! A plugin never holds the project model. Every change is a Command API call,
//! so a plugin edit is an ordinary undoable command that the GUI, the CLI and
//! the MCP bridge all see identically (docs/PLAN.md §3, §6).
//!
//! [`Project`] is the handle: [`Project::run`] applies one undoable command,
//! [`Project::query`] reads without mutating, and both take a typed parameter
//! struct from [`params`] that carries its own method name. [`Project::run_json`]
//! and [`Project::query_json`] take a `serde_json::Value` for the methods the
//! SDK does not type.
//!
//! Times cross as [`RationalTime`] — a tick count and an exact fractional rate,
//! never seconds — and [`time`] does the arithmetic without ever reaching for a
//! float. Identifiers cross as distinct records, so a [`TrackId`] cannot be
//! passed where a [`ClipId`] is expected; [`Id`] gives all six the same
//! constructor and accessor.
//!
//! # Failures
//!
//! Everything returns [`Result`], whose error is the [`Error`] record: a stable
//! `code`, a one-line message and details carrying the item it belongs to and a
//! hint saying what would fix it. [`errors`] is the catalogue — the code
//! constants to match on, what each one means, and its hint — so a plugin
//! author never has to read a message to tell two failures apart.
//!
//! # One example per world
//!
//! Nine worlds, nine features, one enabled at a time. Each sketch below is the
//! whole of the plugin's contract with the host; everything else it does goes
//! through [`Project`].
//!
//! ## `command` (default) — run one Command API call
//!
//! ```ignore
//! // subordinate-sdk = "0.1"
//! use subordinate_sdk::{Guest, Project, ProjectId, Result, export, json, params};
//!
//! struct Plugin;
//! impl Guest for Plugin {
//!     fn run(project: ProjectId, args: String) -> Result<String> {
//!         let project = Project::new(project);
//!         let history = project.query(&params::HistoryGet {})?;
//!         // `json!` and `serde_json` are re-exported by the SDK, so a plugin's
//!         // `Cargo.toml` never grows a JSON dependency of its own.
//!         Ok(json!({ "can_undo": history.can_undo }).to_string())
//!     }
//! }
//! export!(Plugin);
//! ```
//!
//! ## `effect` — declare a video effect and its shader
//!
//! ```ignore
//! // features = ["effect"], default-features = false
//! use subordinate_sdk::bindings::exports::*;  // world exports live at the root
//! use subordinate_sdk::{EffectDesc, Guest, export};
//!
//! struct Plugin;
//! impl Guest for Plugin {
//!     fn describe() -> EffectDesc {
//!         EffectDesc {
//!             id: "example:tint".to_owned(),
//!             name: "Tint".to_owned(),
//!             params: Vec::new(),
//!             shader: include_str!("tint.wgsl").to_owned(),
//!             entry_point: "main".to_owned(),
//!         }
//!     }
//! }
//! export!(Plugin);
//! ```
//!
//! ## `effect-cpu` — the same, plus a deliberately slow CPU path
//!
//! ```ignore
//! // features = ["effect-cpu"], default-features = false
//! // `process-cpu` copies a whole frame into the sandbox and back, so it is for
//! // thumbnails, analysis passes and test fixtures — never playback at picture
//! // size (decision-6).
//! impl Guest for Plugin {
//!     fn describe() -> EffectDesc { /* as above */ }
//!     fn process_cpu(mut frame: Frame, params: Vec<ParamBinding>) -> Result<Frame> {
//!         for byte in &mut frame.data { *byte = 255 - *byte; }
//!         Ok(frame)
//!     }
//! }
//! ```
//!
//! ## `commands` — contribute menu entries and shortcuts
//!
//! ```ignore
//! // features = ["commands"], default-features = false
//! // The host opens one undo group around `run`, so however many primitives the
//! // command applies, the user undoes it in one step.
//! impl Guest for Plugin {
//!     fn commands() -> Vec<CommandDesc> {
//!         vec![CommandDesc {
//!             id: "cut-silence".to_owned(),
//!             label: "Cut Silence".to_owned(),
//!             menu_path: vec!["Plugins".to_owned()],
//!             default_shortcut: Some("Ctrl+Shift+S".to_owned()),
//!             args_schema: "{}".to_owned(),
//!         }]
//!     }
//!     fn run(id: String, context: CommandContext) -> Result<String> {
//!         let project = Project::new(context.project);
//!         project.run(&params::RippleDelete { /* … */ })?;
//!         Ok("{}".to_owned())
//!     }
//! }
//! ```
//!
//! ## `mcp-tools` — contribute tools to the agent surface
//!
//! ```ignore
//! // features = ["mcp-tools"], default-features = false
//! // The host prefixes each name with the plugin id and validates arguments
//! // against `json-schema` before the call arrives, so `call` may trust `args`.
//! impl Guest for Plugin {
//!     fn tools() -> Vec<ToolDesc> {
//!         vec![ToolDesc {
//!             name: "clip_count".to_owned(),
//!             description: "How many clips a sequence holds.".to_owned(),
//!             json_schema: r#"{"type":"object","properties":{"sequence":{"type":"string"}}}"#.to_owned(),
//!         }]
//!     }
//!     fn call(name: String, args_json: String) -> Result<String> {
//!         use subordinate_sdk::{json, serde_json};
//!         let args: serde_json::Value = serde_json::from_str(&args_json).unwrap_or_default();
//!         Ok(json!({ "tool": name }).to_string())
//!     }
//! }
//! ```
//!
//! ## `audio-effect` — process blocks of samples
//!
//! ```ignore
//! // features = ["audio-effect"], default-features = false
//! // Audio is the one place floats are right. Keep ramp state between calls and
//! // reuse the incoming buffer: the host times every block and bypasses a plugin
//! // that repeatedly overruns its share of real time.
//! impl Guest for Plugin {
//!     fn describe() -> EffectDescription { /* name, params, real-time share */ }
//!     fn process(mut block: Vec<f32>, channels: u32, rate: u32, params: Vec<Param>)
//!         -> Result<Vec<f32>>
//!     {
//!         for sample in &mut block { *sample *= 0.5; }
//!         Ok(block)
//!     }
//! }
//! ```
//!
//! ## `importer` — read a file into media and sequence specifications
//!
//! ```ignore
//! // features = ["importer"], default-features = false
//! // The world exports an interface rather than bare functions, so the trait to
//! // implement is `bindings::exports::subordinate::plugin::importer_api::Guest`.
//! use subordinate_sdk::bindings::exports::subordinate::plugin::importer_api::Guest;
//!
//! impl Guest for Plugin {
//!     fn supported_extensions() -> Vec<String> { vec!["otio".to_owned()] }
//!     fn import(path: String) -> Result<Vec<MediaOrSequenceSpec>> {
//!         // The host applies the specs itself, as one undoable import.
//!         Ok(Vec::new())
//!     }
//! }
//! ```
//!
//! ## `exporter` — contribute presets and post-export actions
//!
//! ```ignore
//! // features = ["exporter"], default-features = false
//! use subordinate_sdk::bindings::exports::subordinate::plugin::exporter_api::Guest;
//!
//! impl Guest for Plugin {
//!     fn presets() -> Vec<PresetDesc> { vec![/* … */] }
//!     fn post_export(path: String) -> Result<()> { Ok(()) }
//! }
//! ```
//!
//! ## `analyzer` — study one media item in the background
//!
//! ```ignore
//! // features = ["analyzer"], default-features = false
//! // An analyzer never edits the project: it reports findings, and the host
//! // decides what to do with them. Call `report-progress` as it goes and honour
//! // `is-cancelled`, because the whole run is one cancellable host job.
//! use subordinate_sdk::bindings::subordinate::plugin::analysis_host;
//!
//! impl Guest for Plugin {
//!     fn analyze(media: MediaId, options: String) -> Result<AnalysisResult> {
//!         for step in 0..10u64 {
//!             if analysis_host::is_cancelled() { break; }
//!             analysis_host::report_progress(step, 10);
//!         }
//!         Ok(AnalysisResult { kind: "silence".to_owned(), markers: vec![], ranges: vec![], data: "{}".to_owned() })
//!     }
//! }
//! ```

pub mod bindings;
pub mod errors;
pub mod host;
pub mod ids;
pub mod params;
pub mod time;
mod wire;

pub use bindings::export;
pub use bindings::subordinate::plugin::command_api;
pub use bindings::subordinate::plugin::command_api::{
    ClipMetadata, LogLevel, MarkerMetadata, ProjectMetadata, Resolution, SequenceMetadata,
    TrackKind, TrackMetadata,
};
pub use bindings::subordinate::plugin::types;
pub use bindings::subordinate::plugin::types::{
    ClipId, Detail, Error, MarkerId, MediaId, ProjectId, Rational, RationalTime, SequenceId,
    TimeRange, TrackId,
};
pub use host::{
    Project, Result, debug, error, error_log, info, log, open_projects, trace, warn, with_detail,
};
pub use ids::Id;
pub use params::CommandParams;

// Every world's answer is a JSON document, and `run_json`/`query_json` take and
// return `serde_json` types, so JSON is part of this SDK's public surface.
// Re-exporting the crate — and its `json!` macro — is what keeps a plugin's
// `Cargo.toml` at one dependency: build an answer with
// `subordinate_sdk::json!({ … }).to_string()` rather than with `format!` over
// hand-written braces, and parse `args` with `subordinate_sdk::serde_json`.
pub use serde_json;
pub use serde_json::json;

// The importer and exporter worlds export an interface rather than bare
// functions, so their guest trait lives under `bindings::exports::…` and there
// is no `Guest` at the bindings root to re-export.
/// The trait a plugin implements for the world this build selected.
#[cfg(not(any(feature = "importer", feature = "exporter")))]
pub use bindings::Guest;

#[cfg(test)]
mod tests {
    use super::{ClipId, Id, ProjectId, TrackId, time};

    /// A plugin only has to implement `Guest`; `export!` then wires it into the
    /// component's exports. Implementing it here keeps the trait's shape under
    /// test on the host triple, where the imports are never called.
    #[cfg(feature = "command")]
    struct Plugin;

    #[cfg(feature = "command")]
    impl super::Guest for Plugin {
        fn run(project: ProjectId, args: String) -> super::Result<String> {
            Ok(format!("{} {args}", project.value))
        }
    }

    #[cfg(feature = "command")]
    #[test]
    fn the_guest_trait_takes_typed_identifiers_and_returns_json() {
        use super::Guest as _;
        assert_eq!(
            Plugin::run(ProjectId::parse("p"), "{}".to_owned()).unwrap(),
            "p {}"
        );
    }

    /// A plugin builds its JSON answer out of what the SDK re-exports, so its
    /// `Cargo.toml` stays at one dependency however involved the answer gets.
    #[test]
    fn json_is_reachable_through_the_sdk_without_a_dependency_of_its_own() {
        let answer = crate::json!({ "sequences": 2, "args_bytes": 0 }).to_string();
        assert_eq!(answer, r#"{"args_bytes":0,"sequences":2}"#);

        let parsed: crate::serde_json::Value =
            crate::serde_json::from_str(&answer).expect("the answer is JSON");
        assert_eq!(parsed["sequences"], 2);
    }

    #[test]
    fn identifiers_are_distinct_types_and_time_carries_a_fractional_rate() {
        let clip = ClipId::parse("c");
        let track = TrackId::parse("t");
        // Distinct records: `clip` could not be passed where `track` is.
        assert_ne!(clip.as_str(), track.as_str());

        let instant = time::frames(24, time::rate::FPS_23_976);
        assert_eq!(instant.rate.denominator, 1_001);
        assert!(time::equals(
            instant,
            time::frames(48, time::rate::new(48_000, 1_001))
        ));
    }
}
