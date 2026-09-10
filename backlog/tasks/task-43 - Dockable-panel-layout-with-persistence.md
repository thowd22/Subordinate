---
id: TASK-43
title: Dockable panel layout with persistence
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 08:07'
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
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers docking: a committed snapshot of the default layout, and a test that a saved layout round-trips through persistence
<!-- AC:END -->
