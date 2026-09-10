//! Guest bindings for the `command` world of `subordinate:plugin@0.1.0`.
//!
//! The expansion carries no doc comments beyond the ones written in the WIT
//! and its lifting code is not `clippy::pedantic` clean, so the two lint
//! groups are switched off for the expansion only: everything this crate
//! writes by hand stays under the workspace lints.

#![allow(missing_docs, clippy::all, clippy::pedantic)]

wit_bindgen::generate!({
    path: "../../../wit",
    world: "command",
    default_bindings_module: "crate::bindings",
});
