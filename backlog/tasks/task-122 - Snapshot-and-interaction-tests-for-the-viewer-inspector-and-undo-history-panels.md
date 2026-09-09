---
id: TASK-122
title: >-
  Snapshot and interaction tests for the viewer, inspector and undo history
  panels
status: To Do
assignee: []
created_date: '2026-09-09 18:21'
labels:
  - ui
  - test
milestone: m-2
dependencies:
  - TASK-119
  - TASK-22
  - TASK-37
  - TASK-42
priority: medium
ordinal: 142000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Viewer shows a rendered frame from the software compositor; the inspector edits clip parameters; the history panel lists commands. Cover all three once the inspector lands.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Snapshots: viewer showing frame 10 of the sample project with timecode overlay, inspector for a selected clip, history panel after three edits
- [ ] #2 Interaction tests: scrub bar drag changes the playhead, editing opacity in the inspector commits one command, clicking a history entry jumps state
<!-- AC:END -->
