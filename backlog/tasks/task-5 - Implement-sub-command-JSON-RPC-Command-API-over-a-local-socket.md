---
id: TASK-5
title: 'Implement sub-command: JSON-RPC Command API over a local socket'
status: Done
assignee: []
created_date: '2026-09-08 20:53'
updated_date: '2026-09-11 13:41'
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
- [x] #1 JSON-RPC 2.0 server exposes every sub-edit command plus project open/save
- [x] #2 Clients can subscribe to change events
- [x] #3 Schema for all methods is exported as JSON Schema for the MCP bridge and SDK to consume
<!-- AC:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Aggregate task: all dotted subtasks are Done and verified individually (see their final summaries); CI green on all three OSes.
<!-- SECTION:FINAL_SUMMARY:END -->
