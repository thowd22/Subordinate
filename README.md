# Subordinate

A cross-platform video editor with a small Rust core and an agent-friendly plugin system.

- Plan: [docs/PLAN.md](docs/PLAN.md)
- Using the editor: [docs/user-guide.md](docs/user-guide.md) — import, editing,
  audio, export, proxies, the pop-out viewer, the keyboard reference and what
  to do when a hardware encoder is missing
- Writing a plugin: [docs/plugin-guide.md](docs/plugin-guide.md) — every world,
  the manifest, capabilities, the developer loop and the headless test harness
- Driving it from an agent: [docs/mcp-guide.md](docs/mcp-guide.md) —
  `subordinate-mcp` bridges the Command API to MCP; `.mcp.json` setup, every
  tool family and the runbook. The example config is
  [docs/examples/mcp.json](docs/examples/mcp.json)
- Tasks, milestones and decisions: managed with [Backlog.md](https://github.com/MrLesk/Backlog.md) in `backlog/` (`backlog board`, `backlog browser`)

## Licence

The core crates and binaries are licensed under the GNU GPL-3.0-or-later (see
`LICENSE`). The plugin SDK (`sdk/`) and the plugin interface definitions
(`wit/`) are licensed under MIT OR Apache-2.0, so plugins built against them
may use any licence, including closed source.
