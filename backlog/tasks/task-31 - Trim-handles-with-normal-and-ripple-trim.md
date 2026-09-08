---
id: TASK-31
title: Trim handles with normal and ripple trim
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
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
<!-- AC:END -->
