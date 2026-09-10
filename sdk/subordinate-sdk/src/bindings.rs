//! Guest bindings generated from `wit/subordinate-plugin.wit`.
//!
//! [`wit_bindgen::generate!`] turns one world into free functions for its host
//! imports, a `Guest` trait for the plugin to implement, and the `export!`
//! macro that wires that implementation into the component's exports. A plugin
//! crate writes:
//!
//! ```ignore
//! struct Plugin;
//!
//! impl subordinate_sdk::Guest for Plugin {
//!     fn run(project: subordinate_sdk::ProjectId, args: String)
//!         -> Result<String, subordinate_sdk::Error> { Ok("{}".to_owned()) }
//! }
//!
//! subordinate_sdk::export!(Plugin);
//! ```
//!
//! # One world per build
//!
//! A component implements exactly one world, so the SDK generates exactly one:
//! each world is a cargo feature, `command` is the default, and a plugin
//! targeting another world turns the default off and names its own:
//!
//! ```toml
//! subordinate-sdk = { version = "0.1", default-features = false, features = ["audio-effect"] }
//! ```
//!
//! Enabling none, or more than one, is a compile error rather than a component
//! the host cannot load: two sets of bindings in one module would leave two
//! world descriptions in the WASM binary and the componentiser would then
//! demand the exports of both.
//!
//! Because every world imports `command-api`, and `command-api` uses `types`,
//! the paths `subordinate::plugin::command_api` and
//! `subordinate::plugin::types` exist whichever feature is on. Everything the
//! SDK builds on top of the bindings is written against those two, so the
//! helper layer is identical in all nine worlds.
//!
//! `pub_export_macro` and `default_bindings_module` are set so `export!` works
//! from a plugin crate without it naming this module.
//!
//! The expansion carries no doc comments beyond the ones written in the WIT,
//! and its lifting code is not `clippy::pedantic` clean, so the two lint groups
//! are switched off for the expansion only. Everything the SDK writes by hand
//! stays under the workspace lints.

#![allow(missing_docs, clippy::all, clippy::pedantic)]

/// The number of world features enabled. Exactly one world may be generated.
const WORLDS: u8 = cfg!(feature = "command") as u8
    + cfg!(feature = "effect") as u8
    + cfg!(feature = "effect-cpu") as u8
    + cfg!(feature = "commands") as u8
    + cfg!(feature = "mcp-tools") as u8
    + cfg!(feature = "audio-effect") as u8
    + cfg!(feature = "importer") as u8
    + cfg!(feature = "exporter") as u8
    + cfg!(feature = "analyzer") as u8;

const _: () = assert!(
    WORLDS == 1,
    "subordinate-sdk generates exactly one WIT world: enable exactly one of the \
     command, effect, effect-cpu, commands, mcp-tools, audio-effect, importer, \
     exporter and analyzer features (default-features = false turns off command)"
);

// The plugin runs one Command API call and returns a JSON document.
#[cfg(feature = "command")]
wit_bindgen::generate!({
    path: "../../wit",
    world: "command",
    pub_export_macro: true,
    default_bindings_module: "subordinate_sdk::bindings",
});

// The plugin describes a video effect and ships a WGSL shader.
#[cfg(feature = "effect")]
wit_bindgen::generate!({
    path: "../../wit",
    world: "effect",
    pub_export_macro: true,
    default_bindings_module: "subordinate_sdk::bindings",
});

// The `effect` world plus the optional, deliberately slow CPU path.
#[cfg(feature = "effect-cpu")]
wit_bindgen::generate!({
    path: "../../wit",
    world: "effect-cpu",
    pub_export_macro: true,
    default_bindings_module: "subordinate_sdk::bindings",
});

// The plugin contributes entries to the Plugins menu and the shortcut map.
#[cfg(feature = "commands")]
wit_bindgen::generate!({
    path: "../../wit",
    world: "commands",
    pub_export_macro: true,
    default_bindings_module: "subordinate_sdk::bindings",
});

// The plugin contributes MCP tools to the agent surface.
#[cfg(feature = "mcp-tools")]
wit_bindgen::generate!({
    path: "../../wit",
    world: "mcp-tools",
    pub_export_macro: true,
    default_bindings_module: "subordinate_sdk::bindings",
});

// The plugin processes blocks of audio samples.
#[cfg(feature = "audio-effect")]
wit_bindgen::generate!({
    path: "../../wit",
    world: "audio-effect",
    pub_export_macro: true,
    default_bindings_module: "subordinate_sdk::bindings",
});

// The plugin reads a file and returns media and sequence specifications.
#[cfg(feature = "importer")]
wit_bindgen::generate!({
    path: "../../wit",
    world: "importer",
    pub_export_macro: true,
    default_bindings_module: "subordinate_sdk::bindings",
});

// The plugin contributes export presets and post-export actions.
#[cfg(feature = "exporter")]
wit_bindgen::generate!({
    path: "../../wit",
    world: "exporter",
    pub_export_macro: true,
    default_bindings_module: "subordinate_sdk::bindings",
});

// The plugin analyses one media item in the background.
#[cfg(feature = "analyzer")]
wit_bindgen::generate!({
    path: "../../wit",
    world: "analyzer",
    pub_export_macro: true,
    default_bindings_module: "subordinate_sdk::bindings",
});
