---
id: TASK-71
title: Autosave and snapshot history
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - core
  - ui
milestone: m-5
dependencies:
  - TASK-3.6
  - TASK-12
references:
  - docs/PLAN.md
priority: medium
ordinal: 92000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Crash recovery and cheap versioning.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Autosave writes to the sidecar dir every N seconds after changes without blocking the UI
- [ ] #2 On open, a newer autosave than the project file prompts to recover
- [ ] #3 Snapshots keep the last K autosaves and can be restored from a menu
<!-- AC:END -->
