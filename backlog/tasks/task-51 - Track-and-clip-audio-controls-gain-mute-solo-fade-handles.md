---
id: TASK-51
title: 'Track and clip audio controls: gain, mute, solo, fade handles'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - ui
  - audio
milestone: m-3
dependencies:
  - TASK-48
  - TASK-31
references:
  - docs/PLAN.md
priority: medium
ordinal: 72000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Basic audio manipulation UI (§2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Audio clips show fade handles at both ends draggable on the timeline
- [ ] #2 Track headers gain mute and solo; inspector shows clip gain in dB
- [ ] #3 All changes commit undoable commands and apply live
<!-- AC:END -->
