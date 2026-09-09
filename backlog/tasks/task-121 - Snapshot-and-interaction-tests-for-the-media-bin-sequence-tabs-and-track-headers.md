---
id: TASK-121
title: >-
  Snapshot and interaction tests for the media bin, sequence tabs and track
  headers
status: To Do
assignee: []
created_date: '2026-09-09 18:21'
labels:
  - ui
  - test
milestone: m-2
dependencies:
  - TASK-119
  - TASK-35
  - TASK-33
  - TASK-34
priority: medium
ordinal: 141000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
These panels are already merged without UI tests. Lock in their layouts and the undoable actions they expose.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Snapshots: bin in list and grid view with the sample project, sequence tab strip with three sequences, track headers with a muted and a locked track
- [ ] #2 Interaction tests: rename a bin, switch sequence tab, toggle mute and lock; each asserts state through the Command API and undo restores it
<!-- AC:END -->
