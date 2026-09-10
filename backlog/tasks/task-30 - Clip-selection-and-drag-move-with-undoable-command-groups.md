---
id: TASK-30
title: Clip selection and drag-move with undoable command groups
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 08:07'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-29
  - TASK-4.2
references:
  - docs/PLAN.md
priority: high
ordinal: 51000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Moving clips is the most frequent edit; it must feel immediate yet produce a single undo step.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Click, shift-click and marquee selection across tracks
- [ ] #2 Dragging moves selected clips with live preview and commits one grouped MoveClip command on release
- [ ] #3 Dragging onto a locked track or outside the sequence is refused visually
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers this: an interaction test that selects and drags a clip and asserts the resulting command group, plus a snapshot showing the selected state
<!-- AC:END -->
