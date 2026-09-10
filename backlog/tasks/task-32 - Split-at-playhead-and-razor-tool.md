---
id: TASK-32
title: Split at playhead and razor tool
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 08:07'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-30
references:
  - docs/PLAN.md
priority: high
ordinal: 53000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Cuts are the basic verb of editing.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Ctrl+K splits selected clips (or all clips under the playhead when nothing is selected)
- [ ] #2 Razor tool splits the clicked clip at the cursor time
- [ ] #3 Split respects snapping and is undoable
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers this: an interaction test that splits at the playhead and asserts the two resulting clips, plus a snapshot of the razor tool state
<!-- AC:END -->
