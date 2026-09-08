---
id: TASK-15
title: 'Frame-accurate seek: keyframe seek then decode-forward'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-14
references:
  - docs/PLAN.md
priority: high
ordinal: 36000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Scrubbing and split-at-playhead demand exact frames on long-GOP sources; naive seeks land on keyframes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 seek_to(time) performs a flushing keyframe-backward seek then discards frames until PTS >= target
- [ ] #2 Seeking within the current GOP forward does not re-seek
- [ ] #3 Test suite asserts the burned-in timecode in fixtures matches the requested frame for 50 random targets including the last frame
<!-- AC:END -->
