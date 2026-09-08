---
id: TASK-43
title: Dockable panel layout with persistence
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-22
  - TASK-28
references:
  - docs/PLAN.md
priority: medium
ordinal: 64000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Panels: bin, timeline, viewer, inspector, export (§5.7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 egui_dock (or equivalent) hosts all panels with drag-to-rearrange and tab groups
- [ ] #2 Layout persists per user and resets via a menu item
- [ ] #3 Default layout matches the plan's panel list
<!-- AC:END -->
