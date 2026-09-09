//! MCP stdio bridge.
//!
//! Speaks the Model Context Protocol over stdio and forwards tool calls to the
//! running app's Command API socket, launching `subordinate-cli serve` when no
//! editor is running. See the [`subordinate_mcp`] library for the parts.
//!
//! stdout carries the protocol and nothing else; every diagnostic goes to
//! stderr, which is where an MCP client shows a server's log.

use std::process::ExitCode;
use std::sync::Arc;

use rmcp::ServiceExt as _;
use sub_core::{SubError, SubResult};
use subordinate_mcp::backend::{Backend, Options};
use subordinate_mcp::bridge::Bridge;
use subordinate_mcp::codes;
use subordinate_mcp::tools::ToolSet;

fn main() -> ExitCode {
    // stdout carries the MCP protocol; `sub_core::logging` writes to stderr.
    if let Err(err) = sub_core::logging::init("info") {
        eprintln!("{err}");
        return ExitCode::FAILURE;
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "subordinate-mcp starting"
    );
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(code = error.code.as_str(), "{error}");
            eprintln!("{}", error.to_json());
            ExitCode::FAILURE
        }
    }
}

/// Connects to the editor and serves MCP over stdio until the client leaves.
fn run() -> SubResult<()> {
    let tools = Arc::new(ToolSet::committed()?);
    let backend = Arc::new(Backend::connect(&Options::from_env())?);
    tracing::info!(
        tools = tools.len(),
        address = %backend.address().to_wire(),
        launched = backend.launched_server(),
        "bridging the Command API"
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            SubError::new(codes::SESSION_FAILED, "the runtime could not be started")
                .with_cause(&error)
        })?;
    runtime.block_on(async move {
        let service = Bridge::new(tools, backend)
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|error| {
                SubError::new(
                    codes::SESSION_FAILED,
                    "the MCP session could not be started",
                )
                .with_cause(&error)
            })?;
        service.waiting().await.map_err(|error| {
            SubError::new(codes::SESSION_FAILED, "the MCP session ended badly").with_cause(&error)
        })?;
        Ok(())
    })
}
