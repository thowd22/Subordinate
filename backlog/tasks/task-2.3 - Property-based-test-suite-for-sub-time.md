---
id: TASK-2.3
title: Property-based test suite for sub-time
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
  - test
milestone: m-0
dependencies:
  - TASK-2.2
references:
  - docs/PLAN.md
parent_task_id: TASK-2
priority: medium
ordinal: 18000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Rational math bugs surface on odd inputs; proptest catches them cheaply before the timeline depends on them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 proptest strategies generate RationalTime at all supported rates
- [ ] #2 Properties: rescale round-trip is identity when exact, add/sub inverse, ordering is total
- [ ] #3 Suite runs in CI in under 30 seconds
<!-- AC:END -->
