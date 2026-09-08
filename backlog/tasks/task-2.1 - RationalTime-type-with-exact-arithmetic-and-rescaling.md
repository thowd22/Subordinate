---
id: TASK-2.1
title: RationalTime type with exact arithmetic and rescaling
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-1.1
references:
  - docs/PLAN.md
parent_task_id: TASK-2
priority: high
ordinal: 16000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
All timeline math uses integer rational time so edits never drift (§5.1).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 RationalTime { value: i64, rate: Rational } supports add, sub, neg, compare, min, max with no floating point
- [ ] #2 rescaled_to(rate) is exact when representable and documents its rounding mode otherwise
- [ ] #3 TimeRange with start and duration supports contains, overlaps, clamp and intersection
- [ ] #4 Unit tests cover mixed-rate arithmetic including 24000/1001
<!-- AC:END -->
