---
id: TASK-5.2
title: 'Local socket transport: Unix domain socket and Windows named pipe'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
  - mcp
milestone: m-0
dependencies:
  - TASK-5.1
references:
  - docs/PLAN.md
parent_task_id: TASK-5
priority: high
ordinal: 31000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Out-of-process clients such as the MCP bridge need a local, authenticated channel that works on all three OSes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Server listens on a per-user socket path (XDG runtime dir, macOS temp dir, Windows named pipe) written to a discoverable lock file
- [ ] #2 Multiple concurrent clients are served with newline-delimited JSON framing
- [ ] #3 Stale socket files from crashed instances are cleaned up on start
- [ ] #4 Integration test connects from a second process and runs a command
<!-- AC:END -->
