---
id: TASK-53
title: Waveform generation job and timeline waveform strips
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - audio
  - ui
milestone: m-3
dependencies:
  - TASK-25
  - TASK-45
  - TASK-46
references:
  - docs/PLAN.md
priority: medium
ordinal: 74000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Audio clips are edited visually by their waveform.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Job computes min/max peaks at several zoom levels per media item into the sidecar dir
- [ ] #2 Timeline draws waveforms on audio clips using cached textures
- [ ] #3 Generation is cancellable and resumable like thumbnails
<!-- AC:END -->
