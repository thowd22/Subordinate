---
id: TASK-4.3
title: >-
  Track and sequence commands: add, remove, reorder, rename, mute, lock,
  create/delete sequence
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
ordinal: 27000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Multiple sequences and tracks are MVP requirements (§2) and need first-class undoable commands.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 AddTrack, RemoveTrack, ReorderTrack, RenameTrack, SetTrackMuted, SetTrackLocked, CreateSequence, DeleteSequence, RenameSequence, SetSequenceSettings exist with inverses
- [ ] #2 Removing a track with clips is refused unless force is set, and force is undoable
- [ ] #3 Locked tracks reject clip commands with a structured error
<!-- AC:END -->
