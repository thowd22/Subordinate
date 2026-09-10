//! Host bindings generated from `wit/subordinate-plugin.wit`.
//!
//! [`wasmtime::component::bindgen!`] turns each world into a type for
//! instantiating a plugin ([`Command`], [`effect::Effect`],
//! [`effect_cpu::EffectCpu`]) and a `Host` trait per imported interface for the
//! host to implement. The generated items are re-exported from the crate root,
//! so a caller writes `sub_plugin::WitError` rather than reaching through the
//! whole package path.
//!
//! Each world needs its own macro expansion, so the effect worlds live in
//! submodules and reuse the `command` world's types through `with:`. There is
//! therefore exactly one Rust `Error`, one `ProjectId` and one `Host` trait
//! across all three worlds, and one `EffectDesc` across both effect worlds.
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

/// The `effect` world: a plugin that declares WGSL and a parameter schema.
pub mod effect {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "effect",
        // A parameter's range and default are `f32`, so these records are
        // comparable but neither `Eq` nor `Hash`.
        additional_derives: [PartialEq],
        with: {
            "subordinate:plugin/types": crate::bindings::subordinate::plugin::types,
            "subordinate:plugin/command-api": crate::bindings::subordinate::plugin::command_api,
        },
    });
}

/// The `effect` world plus the optional, slow CPU path.
pub mod effect_cpu {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "effect-cpu",
        additional_derives: [PartialEq],
        with: {
            "subordinate:plugin/types": crate::bindings::subordinate::plugin::types,
            "subordinate:plugin/command-api": crate::bindings::subordinate::plugin::command_api,
            "subordinate:plugin/effect-types": crate::bindings::effect::subordinate::plugin::effect_types,
        },
    });
}

/// The richer `commands` world: the same host imports, plus the menu and
/// shortcut registration exports.
///
/// `with` points the generated code at the modules above rather than at a
/// second copy of them, so a `WitError` returned by a `commands` plugin and one
/// returned by a `command` plugin are the same Rust type and one host state can
/// serve both worlds.
pub mod menu {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "commands",
        additional_derives: [PartialEq, Eq, Hash],
        with: {
            "subordinate:plugin/types": super::subordinate::plugin::types,
            "subordinate:plugin/command-api": super::subordinate::plugin::command_api,
        },
    });
}

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
