---
id: TASK-4.4
title: 'Parameter, marker, media and bin commands'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-4.2
  - TASK-4.3
references:
  - docs/PLAN.md
parent_task_id: TASK-4
priority: medium
ordinal: 28000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Inspector edits, markers and bin organisation must be undoable and scriptable like everything else.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 SetClipParams (opacity, transform, gain, fades), AddMarker, MoveMarker, RemoveMarker, ImportMedia, RemoveMedia, RelinkMedia, CreateBin, MoveToBin, RenameBin exist with inverses
- [ ] #2 SetClipParams validates via Clip::validate before applying
- [ ] #3 Tests cover undo for each
<!-- AC:END -->
