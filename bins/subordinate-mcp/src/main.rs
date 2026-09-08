//! MCP stdio bridge.
//!
//! Speaks the Model Context Protocol over stdio and forwards tool calls to
//! the running app's Command API socket (or launches the CLI headlessly).

use std::process::ExitCode;

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
    println!("subordinate-mcp {}", env!("CARGO_PKG_VERSION"));
    ExitCode::SUCCESS
}
