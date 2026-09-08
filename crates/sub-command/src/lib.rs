//! The Command API: JSON-RPC 2.0 dispatch and the local socket server.
//!
//! One internal API serves the GUI, the CLI, the MCP bridge and plugins so
//! the agent surface is exactly as capable as the UI. Also hosts the engine
//! thread that owns project state. See docs/PLAN.md §4 and §7.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_links() {}
}
