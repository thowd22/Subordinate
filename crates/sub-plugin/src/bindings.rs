//! Host bindings generated from `wit/subordinate-plugin.wit`.
//!
//! [`wasmtime::component::bindgen!`] turns the `command` world into a
//! [`Command`] type for instantiating a plugin and a `Host` trait per imported
//! interface for the host to implement. The generated items are re-exported
//! from the crate root, so a caller writes `sub_plugin::WitError` rather than
//! reaching through the whole package path.
//!
//! The `importer` and `exporter` worlds get one submodule each. Both import
//! `command-api`, so their bindgen invocations map that interface and `types`
//! onto the ones generated here with `with`: a host implements the one `Host`
//! trait and links it into any of the three worlds, and a `WitError` from an
//! importer is the same Rust type as a `WitError` from a command plugin.
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

/// The `importer` world: a plugin that turns a file into media items and
/// sequences for the host to apply through the Command API.
pub mod importer {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "importer",
        additional_derives: [PartialEq, Eq, Hash],
        with: {
            "subordinate:plugin/types": crate::bindings::subordinate::plugin::types,
            "subordinate:plugin/command-api": crate::bindings::subordinate::plugin::command_api,
        },
    });
}

/// The `exporter` world: a plugin that contributes export presets and an
/// optional post-export hook.
pub mod exporter {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "exporter",
        additional_derives: [PartialEq, Eq, Hash],
        with: {
            "subordinate:plugin/types": crate::bindings::subordinate::plugin::types,
            "subordinate:plugin/command-api": crate::bindings::subordinate::plugin::command_api,
        },
    });
}
