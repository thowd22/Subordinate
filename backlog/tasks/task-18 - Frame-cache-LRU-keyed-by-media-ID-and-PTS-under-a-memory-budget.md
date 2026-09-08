---
id: TASK-18
title: 'Frame cache: LRU keyed by media ID and PTS under a memory budget'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-17
references:
  - docs/PLAN.md
priority: high
ordinal: 39000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Scrub responsiveness depends on hitting cached frames (§5.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Cache stores decoded frames with a configurable byte budget and LRU eviction
- [ ] #2 Hit and miss counters are exposed; a test proves eviction respects the budget
- [ ] #3 Frames are reference-counted so the compositor can hold one across eviction
<!-- AC:END -->
