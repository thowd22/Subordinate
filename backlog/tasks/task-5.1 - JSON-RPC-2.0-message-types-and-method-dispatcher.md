---
id: TASK-5.1
title: JSON-RPC 2.0 message types and method dispatcher
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
  - mcp
milestone: m-0
dependencies:
  - TASK-12
references:
  - docs/PLAN.md
parent_task_id: TASK-5
priority: high
ordinal: 30000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The Command API is the single surface for GUI, CLI, MCP and plugins (decision-7). Types and dispatch come before transport so they can be unit tested in-process.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Request, Response, Error and Notification types per JSON-RPC 2.0 with batch support
- [ ] #2 Dispatcher maps method names (project.open, timeline.add_clip, etc.) to engine commands and queries with typed params
- [ ] #3 Unknown method and invalid params return the standard JSON-RPC error codes wrapping SubError
<!-- AC:END -->
