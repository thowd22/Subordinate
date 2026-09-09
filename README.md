# Subordinate

A cross-platform video editor with a small Rust core and an agent-friendly plugin system.

- Plan: [docs/PLAN.md](docs/PLAN.md)
- Driving it from an agent: `subordinate-mcp` bridges the Command API to MCP;
  see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#driving-the-editor-from-an-agent-mcp)
  and the `.mcp.json` example in [docs/examples/mcp.json](docs/examples/mcp.json)
- Tasks, milestones and decisions: managed with [Backlog.md](https://github.com/MrLesk/Backlog.md) in `backlog/` (`backlog board`, `backlog browser`)

## Licence

The core crates and binaries are licensed under the GNU GPL-3.0-or-later (see
`LICENSE`). The plugin SDK (`sdk/`) and the plugin interface definitions
(`wit/`) are licensed under MIT OR Apache-2.0, so plugins built against them
may use any licence, including closed source.
