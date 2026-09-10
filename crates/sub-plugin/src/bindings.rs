//! Host bindings generated from `wit/subordinate-plugin.wit`.
//!
//! [`wasmtime::component::bindgen!`] turns the `command` world into a
//! [`Command`] type for instantiating a plugin and a `Host` trait per imported
//! interface for the host to implement. The generated items are re-exported
//! from the crate root, so a caller writes `sub_plugin::WitError` rather than
//! reaching through the whole package path.
//!
//! The macro expansion carries no doc comments of its own beyond the ones
//! written in the WIT, and its `Vec::from_raw_parts` lifting code is not
//! `clippy::pedantic` clean, so the two lint groups are switched off for the
//! expansion only. Everything the crate writes by hand stays under the
//! workspace lints.

#![allow(missing_docs, clippy::all, clippy::pedantic)]

wasmtime::component::bindgen!({
    path: "../../wit",
    world: "command",
    // Every record here is integers and strings, so the comparison derives are
    // free and let a host compare, deduplicate and assert on the records it
    // hands a plugin.
    additional_derives: [PartialEq, Eq, Hash],
});

/// The `mcp-tools` world: a plugin that contributes MCP tools.
///
/// Generated separately because one `bindgen!` expansion covers one world. The
/// `with` map points the shared interfaces at the `command` expansion above, so
/// `WitError` and the `command_api::Host` trait are the same Rust items in both
/// worlds and a host implements them once.
pub mod mcp_tools {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "mcp-tools",
        with: {
            "subordinate:plugin/types": super::subordinate::plugin::types,
            "subordinate:plugin/command-api": super::subordinate::plugin::command_api,
        },
        additional_derives: [PartialEq, Eq, Hash],
    });
}
