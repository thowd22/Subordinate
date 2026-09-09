//! Guest bindings generated from `wit/subordinate-plugin.wit`.
//!
//! [`wit_bindgen::generate!`] turns the `command` world into free functions for
//! the `command-api` host import, a `Guest` trait for the plugin to implement,
//! and the `export!` macro that wires that implementation into the component's
//! exports. A plugin crate writes:
//!
//! ```ignore
//! struct Plugin;
//!
//! impl subordinate_sdk::Guest for Plugin {
//!     fn run(project: subordinate_sdk::ProjectId, args: String)
//!         -> Result<String, subordinate_sdk::Error> { Ok("{}".to_owned()) }
//! }
//!
//! subordinate_sdk::bindings::export!(Plugin);
//! ```
//!
//! `pub_export_macro` and `default_bindings_module` are set so that macro works
//! from another crate without the plugin naming this module twice.
//!
//! The expansion carries no doc comments beyond the ones written in the WIT,
//! and its lifting code is not `clippy::pedantic` clean, so the two lint groups
//! are switched off for the expansion only. Everything the SDK writes by hand
//! stays under the workspace lints.

#![allow(missing_docs, clippy::all, clippy::pedantic)]

wit_bindgen::generate!({
    path: "../../wit",
    world: "command",
    pub_export_macro: true,
    default_bindings_module: "subordinate_sdk::bindings",
});
