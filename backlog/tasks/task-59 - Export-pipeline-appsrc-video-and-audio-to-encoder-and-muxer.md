---
id: TASK-59
title: 'Export pipeline: appsrc video and audio to encoder and muxer'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - export
milestone: m-4
dependencies:
  - TASK-58
  - TASK-55
  - TASK-57
  - TASK-8
references:
  - docs/PLAN.md
priority: high
ordinal: 80000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Core of phase 4: turn a sequence into a file (§5.5).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Pipeline builds appsrc (video) and appsrc (audio) into the selected encoders and an mp4, mkv or mov muxer with correct timestamps
- [ ] #2 Offline audio render feeds the audio appsrc in lockstep with video
- [ ] #3 Output duration equals sequence duration within one frame; audio and video are in sync in the output (verified by test with burned-in timecode and a click track)
<!-- AC:END -->
