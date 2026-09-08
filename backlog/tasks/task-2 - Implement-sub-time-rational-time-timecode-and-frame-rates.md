---
id: TASK-2
title: 'Implement sub-time: rational time, timecode and frame rates'
status: To Do
assignee: []
created_date: '2026-09-08 20:53'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-1
references:
  - docs/PLAN.md
priority: high
ordinal: 2000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
All timeline math uses RationalTime { value: i64, rate } and never floats (PLAN.md §5.1). Drop-frame timecode at 29.97 and 59.94 must be handled correctly since NTSC sources are common.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 RationalTime supports add, subtract, rescale between rates and comparison without floating point
- [ ] #2 Timecode formatting and parsing round-trips for 24, 25, 29.97 DF/NDF, 30, 50, 59.94 DF/NDF, 60 fps
- [ ] #3 Property-based tests cover rescale round-trips
<!-- AC:END -->
