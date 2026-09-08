---
id: TASK-55
title: Offline audio render through the mixer for export
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - audio
  - export
milestone: m-3
dependencies:
  - TASK-48
references:
  - docs/PLAN.md
priority: high
ordinal: 76000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Export must produce identical mixes to playback without real-time constraints.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 render_audio(sequence, range) produces interleaved PCM deterministically using the same mixer code
- [ ] #2 Two renders of the same project are bit-identical
- [ ] #3 Test compares a rendered fade against expected gain curve
<!-- AC:END -->
