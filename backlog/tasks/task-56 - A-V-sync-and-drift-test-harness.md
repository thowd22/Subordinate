---
id: TASK-56
title: A/V sync and drift test harness
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - test
  - audio
milestone: m-3
dependencies:
  - TASK-50
references:
  - docs/PLAN.md
priority: medium
ordinal: 77000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Phase 3 exit criterion must be measurable.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Harness plays the long fixture headlessly, sampling audio position against the displayed frame's PTS every second
- [ ] #2 Reports maximum drift in frames; CI asserts under one frame on Linux
- [ ] #3 Results appended to docs/PERFORMANCE.md
<!-- AC:END -->
