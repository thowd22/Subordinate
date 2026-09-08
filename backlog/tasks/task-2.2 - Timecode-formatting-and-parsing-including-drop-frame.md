---
id: TASK-2.2
title: Timecode formatting and parsing including drop-frame
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-2.1
references:
  - docs/PLAN.md
parent_task_id: TASK-2
priority: high
ordinal: 17000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
NTSC-rate sources are common and drop-frame timecode is a classic source of off-by-one frame errors.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Format and parse HH:MM:SS:FF and HH;MM;SS;FF for 23.976, 24, 25, 29.97 DF and NDF, 30, 50, 59.94 DF and NDF, 60
- [ ] #2 Round-trip property tests over the full 24-hour range pass for every listed rate
- [ ] #3 Known drop-frame vectors (e.g. frame 17982 at 29.97 DF is 00:10:00;00) are asserted explicitly
<!-- AC:END -->
