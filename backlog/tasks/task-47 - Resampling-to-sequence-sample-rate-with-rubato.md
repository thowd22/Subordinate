---
id: TASK-47
title: Resampling to sequence sample rate with rubato
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-45
  - TASK-46
references:
  - docs/PLAN.md
priority: high
ordinal: 68000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sources at 44.1 kHz and 48 kHz mix on one timeline.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Per-source resampler converts to the sequence rate with a fixed latency reported to the mixer
- [ ] #2 Resampler runs off the audio thread and feeds a ring buffer
- [ ] #3 Test: a 1 kHz tone at 44.1 kHz resampled to 48 kHz has the expected frequency and no clicks at chunk boundaries
<!-- AC:END -->
