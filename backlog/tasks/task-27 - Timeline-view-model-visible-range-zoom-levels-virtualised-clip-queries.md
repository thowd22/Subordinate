---
id: TASK-27
title: 'Timeline view model: visible range, zoom levels, virtualised clip queries'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-12
references:
  - docs/PLAN.md
priority: high
ordinal: 48000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A timeline with hundreds of clips must only lay out what is visible (§5.7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 View model maps pixels to RationalTime and back for a zoom range from whole sequence to single frame
- [ ] #2 Query returns only clips intersecting the visible range per track
- [ ] #3 Unit tests cover zoom math at extreme levels
<!-- AC:END -->
