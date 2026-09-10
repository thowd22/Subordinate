---
id: TASK-40
title: 'Markers: add, move, remove, rename with timeline and ruler display'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 08:07'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-29
  - TASK-4.4
references:
  - docs/PLAN.md
priority: medium
ordinal: 61000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Markers are the simplest way for agents and analyzers to annotate a timeline.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 M adds a marker at the playhead; markers render on the ruler with names and colours
- [ ] #2 Drag moves, double-click renames, Delete removes; all undoable
- [ ] #3 Markers are snap targets
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers markers: a committed snapshot of the ruler and timeline with markers, and an interaction test that adds, moves or renames one and asserts the command
<!-- AC:END -->
