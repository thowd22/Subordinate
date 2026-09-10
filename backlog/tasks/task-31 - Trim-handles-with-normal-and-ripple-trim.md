---
id: TASK-31
title: Trim handles with normal and ripple trim
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
ordinal: 52000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Trimming in and out points is core editing; ripple trim keeps downstream clips contiguous.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Edge hover shows trim cursor; drag trims in or out point clamped to source range
- [ ] #2 Holding a modifier performs ripple trim shifting later clips on the track
- [ ] #3 Trim commits TrimClipIn/Out (grouped with moves for ripple) and is undoable
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers this: an interaction test that drags a trim handle and asserts the trimmed times, plus a snapshot showing the handles
<!-- AC:END -->
