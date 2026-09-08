---
id: decision-7
title: 'Single Command API over a local socket serves GUI, CLI, MCP and plugins'
date: '2026-09-08 20:52'
status: accepted
---
## Context

GUI, CLI, MCP server and plugins all need to perform edits. Separate code paths would drift and would make the agent surface weaker than the UI.

## Decision

One internal Command API (JSON-RPC 2.0 over a Unix socket or named pipe) exposes every undoable command. GUI, subordinate-cli, subordinate-mcp and WASM plugins all go through it.

## Consequences

Anything the UI can do, an agent or plugin can do. The MCP bridge is a thin separate binary. Change events are broadcast to subscribers.
