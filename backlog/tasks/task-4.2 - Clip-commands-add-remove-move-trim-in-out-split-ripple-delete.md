---
id: TASK-4.2
title: 'Clip commands: add, remove, move, trim in/out, split, ripple delete'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-4.1
references:
  - docs/PLAN.md
parent_task_id: TASK-4
priority: high
ordinal: 26000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
These are the primitive edits the timeline UI, agents and plugins compose (§2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 AddClip, RemoveClip, MoveClip, TrimClipIn, TrimClipOut, SplitClip and RippleDelete exist with inverses
- [ ] #2 Overlap resolution policy (overwrite trims or removes covered clips) is implemented and documented
- [ ] #3 Split at a boundary or outside the clip returns a structured error, not a panic
- [ ] #4 Tests cover each command plus undo for edge cases at clip boundaries
<!-- AC:END -->
