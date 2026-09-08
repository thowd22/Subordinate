---
id: TASK-3.2
title: 'Per-clip parameters: source range, placement, opacity, transform, gain, fades'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-3.1
references:
  - docs/PLAN.md
parent_task_id: TASK-3
priority: high
ordinal: 20000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The MVP supports a fixed set of clip parameters (§2, §5.1); modelling them explicitly keeps the compositor and mixer simple.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Clip has source_range and timeline placement as TimeRange, plus opacity, position/scale/rotation transform, gain in dB, fade in/out durations
- [ ] #2 Invariants (non-negative durations, fades not exceeding clip length) are validated by a validate() method returning SubError
- [ ] #3 Unit tests cover valid and invalid parameter sets
<!-- AC:END -->
