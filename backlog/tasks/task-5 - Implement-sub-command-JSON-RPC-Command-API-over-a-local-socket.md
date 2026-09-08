---
id: TASK-5
title: 'Implement sub-command: JSON-RPC Command API over a local socket'
status: To Do
assignee: []
created_date: '2026-09-08 20:53'
labels:
  - core
  - mcp
milestone: m-0
dependencies:
  - TASK-4
references:
  - docs/PLAN.md
priority: high
ordinal: 5000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
One internal Command API serves the GUI, CLI, MCP bridge and plugins (decision-7). Unix domain socket on Linux/macOS, named pipe on Windows. Change events are broadcast to subscribers so the UI and MCP clients stay in sync.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 JSON-RPC 2.0 server exposes every sub-edit command plus project open/save
- [ ] #2 Clients can subscribe to change events
- [ ] #3 Schema for all methods is exported as JSON Schema for the MCP bridge and SDK to consume
<!-- AC:END -->
