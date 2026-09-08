---
id: TASK-49
title: cpal output stream with device selection and format negotiation
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-48
references:
  - docs/PLAN.md
priority: high
ordinal: 70000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Audio must play on ALSA/PipeWire, WASAPI and CoreAudio.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Output opens the default device at the sequence rate or the closest supported rate with resampling
- [ ] #2 Settings panel lists devices; switching reopens the stream without crashing
- [ ] #3 Underruns are counted and surfaced in diagnostics
<!-- AC:END -->
