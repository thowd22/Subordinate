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

/// Host bindings for the `audio-effect` world.
///
/// A second [`wasmtime::component::bindgen!`] expansion, because one macro
/// generates one world. `with` points the shared `types` and `command-api`
/// interfaces at the `command` world's expansion above, so there is exactly one
/// [`super::types::Error`] and one `command_api::Host` trait in the crate: a
/// host that already serves the command world serves this one by adding the
/// audio world's own `add_to_linker`.
///
/// Only the `audio` interface is new here, and its records carry `f32`, so they
/// derive `PartialEq` but not `Eq` or `Hash`.
pub mod audio {
    #![allow(missing_docs, clippy::all, clippy::pedantic)]

    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "audio-effect",
        with: {
            "subordinate:plugin/types": super::subordinate::plugin::types,
            "subordinate:plugin/command-api": super::subordinate::plugin::command_api,
        },
        additional_derives: [PartialEq],
    });
}
