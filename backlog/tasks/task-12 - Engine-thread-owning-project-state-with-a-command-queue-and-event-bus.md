---
id: TASK-12
title: Engine thread owning project state with a command queue and event bus
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-4.4
references:
  - docs/PLAN.md
priority: high
ordinal: 29000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The UI thread must never block and several clients (UI, MCP, plugins) mutate the same project, so a single owner thread serialises commands and broadcasts changes (§4 threading model).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Engine runs on its own thread, accepts commands over a channel and replies with results
- [ ] #2 Every applied command emits a ChangeEvent (entity kind, ID, change type) on a broadcast channel
- [ ] #3 Read access is via immutable snapshots (Arc) so readers never hold a lock across a frame
- [ ] #4 A stress test applies 10k commands from three threads without deadlock or lost events
<!-- AC:END -->
