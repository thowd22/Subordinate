---
id: TASK-23
title: Playback scheduler and clock (video-only for now)
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - media
  - ui
milestone: m-1
dependencies:
  - TASK-22
references:
  - docs/PLAN.md
priority: high
ordinal: 44000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Play/pause needs a clock that advances the playhead and requests frames ahead; the audio clock replaces it in phase 3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Play, pause, JKL shuttle at 1x/2x/4x forward and reverse, loop range
- [ ] #2 Scheduler drops frames rather than stalling when decode falls behind and logs drop counts
- [ ] #3 Playhead position is published as an engine event
<!-- AC:END -->
