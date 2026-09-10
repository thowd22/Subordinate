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

/// The `analyzer` world: background analysis producing markers, ranges or
/// metadata (docs/PLAN.md §6.2, TASK-79).
///
/// A second expansion rather than a second crate, and `with` points its shared
/// interfaces at the ones the `command` world already generated, so
/// `command-api` is one type on both sides and a host implements each `Host`
/// trait once.
pub mod analyzer {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "analyzer",
        additional_derives: [PartialEq, Eq, Hash],
        with: {
            "subordinate:plugin/types": super::subordinate::plugin::types,
            "subordinate:plugin/command-api": super::subordinate::plugin::command_api,
        },
    });
}
