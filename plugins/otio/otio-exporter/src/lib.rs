//! The first-party OpenTimelineIO exporter.
//!
//! # Why the `command` world and not the `exporter` world
//!
//! `exporter` is about *encoding*: it contributes presets to the export panel
//! and a hook that runs once the host has written a media file. Writing an
//! OTIO document is not encoding — nothing is rendered, and the output is a
//! description of the edit rather than a picture of it — so this ships as a
//! `command` plugin: the host runs it against a project, it reads the project
//! back through the Command API and returns the document.
//!
//! ```text
//! subordinate-cli plugin run com.subordinate.otio-export \
//!     --args '{"sequence": "Edit", "path": "/project/edit.otio"}'
//! ```
//!
//! Both arguments are optional: the first sequence is exported when none is
//! named, and the document comes back in the result when no path is given.
//! `path` is inside the folder the manifest granted write access to, which the
//! host mounts at `/project`.
//!
//! Build it with `cargo build --release --target wasm32-wasip2`.

pub mod bindings;
pub mod export;

use otio_core::error::Error;

use crate::bindings::Guest;
use crate::bindings::subordinate::plugin::command_api::{self, LogLevel};
use crate::bindings::subordinate::plugin::types::{Detail, Error as WitError, ProjectId};

/// The `command` world implementation.
pub struct Plugin;

impl Guest for Plugin {
    fn run(project: ProjectId, args: String) -> Result<String, WitError> {
        let answer = command_api::query(&project, "project.get", "{}")?;
        let export = export::export(&answer, &args).map_err(|error| wit_error(&error))?;
        if let Some(path) = &export.path {
            std::fs::write(path, export.document.as_bytes())
                .map_err(|error| wit_error(&export::write_failed(path, &error.to_string())))?;
            command_api::log(
                LogLevel::Info,
                &format!("otio export: wrote {} to {path}", export.sequence_name),
            );
        }
        Ok(export::report(&export))
    }
}

/// The component's exported entry points.
///
/// Only on wasm: an interface export's symbol name is the full WIT name,
/// `subordinate:plugin/...#...`, which a host linker's export list cannot
/// parse. The `rlib` half of this crate is built for the host so the mapping
/// below can be unit-tested, and it needs no component glue to do that.
#[cfg(target_family = "wasm")]
mod component {
    #![allow(
        unsafe_code,
        reason = "the generated export glue is `extern \"C\"` shims with exported symbol names"
    )]

    use super::Plugin;

    crate::bindings::export!(Plugin);
}

/// Carries a failure across the component boundary with its code intact.
fn wit_error(error: &Error) -> WitError {
    WitError {
        code: error.code.to_owned(),
        message: error.message.clone(),
        details: error
            .details
            .iter()
            .map(|(key, value)| Detail {
                key: key.clone(),
                // A detail's value is a JSON fragment, so a string detail is a
                // quoted string.
                value: format!("{value:?}"),
            })
            .collect(),
    }
}
