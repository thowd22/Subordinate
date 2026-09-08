---
id: TASK-52
title: Level meters for tracks and master
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - ui
  - audio
milestone: m-3
dependencies:
  - TASK-49
references:
  - docs/PLAN.md
priority: medium
ordinal: 73000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users need to see levels; meters must be read without locking the audio thread.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Peak and RMS per track and master published via atomics from the callback
- [ ] #2 Meter widget with peak hold and clip indicator in the timeline headers and a master meter in the viewer
- [ ] #3 Meter update costs under 0.1 ms per frame
<!-- AC:END -->
