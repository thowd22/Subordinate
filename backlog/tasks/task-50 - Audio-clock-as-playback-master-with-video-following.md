---
id: TASK-50
title: Audio clock as playback master with video following
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - audio
  - media
milestone: m-3
dependencies:
  - TASK-49
  - TASK-23
references:
  - docs/PLAN.md
priority: high
ordinal: 71000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sync is defined by audio; video frames are chosen from the audio position (§5.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Playhead time derives from samples rendered by the callback plus output latency
- [ ] #2 Video scheduler picks the frame for the audio-derived time; the video-only clock is removed
- [ ] #3 Drift test over 10 minutes of the long fixture stays under one frame
<!-- AC:END -->
