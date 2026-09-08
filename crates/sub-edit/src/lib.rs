//! Editing operations as undoable commands.
//!
//! Every mutation of a project is a `Command` with apply and revert. The
//! same command set is what the Command API, the MCP server and plugins
//! expose, so no editing logic may bypass this crate. See docs/PLAN.md §5.1.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_links() {}
}
