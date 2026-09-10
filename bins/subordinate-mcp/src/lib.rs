//! The MCP stdio bridge: Claude Code's way into the editor.
//!
//! `subordinate-mcp` is a small process an MCP client starts from a `.mcp.json`
//! entry. It speaks the Model Context Protocol over stdin and stdout with
//! [`rmcp`], and forwards every tool call to the editor's Command API over the
//! local socket, so the editor itself stays an ordinary desktop process and the
//! agent surface is exactly the Command API surface (docs/PLAN.md §7).
//!
//! - [`tools`] — the tools, generated from the committed
//!   `docs/schema/command-api.json` so names, descriptions and parameter
//!   schemas cannot drift from the methods they call.
//! - [`backend`] — finding the running editor through its lock file, or
//!   launching `subordinate-cli serve` when none is running.
//! - [`resources`] — the project as readable resources: `project://current`,
//!   `sequence://{id}` and `media://{id}`, with the caching hints that let a
//!   client hold on to them.
//! - [`watch`] — the editor's change events, carried to resource subscribers.
//! - [`bridge`] — the [`bridge::Bridge`] itself, an `rmcp` server handler.
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use std::sync::Arc;
//!
//! use rmcp::ServiceExt as _;
//! use subordinate_mcp::backend::{Backend, Options};
//! use subordinate_mcp::bridge::Bridge;
//! use subordinate_mcp::tools::ToolSet;
//!
//! let tools = Arc::new(ToolSet::committed()?);
//! let backend = Arc::new(Backend::connect(&Options::from_env())?);
//! let service = Bridge::new(tools, backend)
//!     .serve(rmcp::transport::stdio())
//!     .await?;
//! service.waiting().await?;
//! # Ok(())
//! # }
//! ```

pub mod backend;
pub mod bridge;
pub mod resources;
pub mod tools;
pub mod watch;

pub use backend::{Backend, Options};
pub use bridge::Bridge;
pub use resources::Resources;
pub use tools::ToolSet;
pub use watch::Watch;

/// The error codes this binary produces.
///
/// Codes are part of the public contract with agents and plugins: an existing
/// one is never renamed or given a new meaning (see `sub_core::error`).
pub mod codes {
    use sub_core::ErrorCode;

    /// The Command API schema the tools are generated from cannot be read.
    pub const SCHEMA_INVALID: ErrorCode = ErrorCode::from_static("mcp.schema_invalid");
    /// No editor is running and the headless CLI cannot be found to start one.
    pub const CLI_NOT_FOUND: ErrorCode = ErrorCode::from_static("mcp.cli_not_found");
    /// The headless server would not start or did not announce itself.
    pub const LAUNCH_FAILED: ErrorCode = ErrorCode::from_static("mcp.launch_failed");
    /// The MCP session itself failed.
    pub const SESSION_FAILED: ErrorCode = ErrorCode::from_static("mcp.session_failed");
    /// A resource URI names nothing this bridge serves.
    pub const UNKNOWN_RESOURCE: ErrorCode = ErrorCode::from_static("mcp.unknown_resource");
    /// The editor's change events could not be watched for resource updates.
    pub const WATCH_FAILED: ErrorCode = ErrorCode::from_static("mcp.watch_failed");
}

#[cfg(test)]
mod tests {
    use super::codes;

    #[test]
    fn code_constants_are_well_formed() {
        for code in [
            codes::SCHEMA_INVALID,
            codes::CLI_NOT_FOUND,
            codes::LAUNCH_FAILED,
            codes::SESSION_FAILED,
            codes::UNKNOWN_RESOURCE,
            codes::WATCH_FAILED,
        ] {
            assert_eq!(code.domain(), "mcp");
            assert_eq!(
                sub_core::ErrorCode::parse(code.as_str()).as_ref(),
                Ok(&code),
                "{code}",
            );
        }
    }
}
