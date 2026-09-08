//! MCP stdio bridge.
//!
//! Speaks the Model Context Protocol over stdio and forwards tool calls to
//! the running app's Command API socket (or launches the CLI headlessly).

fn main() {
    println!("subordinate-mcp {}", env!("CARGO_PKG_VERSION"));
}
