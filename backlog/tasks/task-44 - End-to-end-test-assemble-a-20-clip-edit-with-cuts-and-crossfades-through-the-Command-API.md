---
id: TASK-44
title: >-
  End-to-end test: assemble a 20-clip edit with cuts and crossfades through the
  Command API
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - test
milestone: m-2
dependencies:
  - TASK-38
  - TASK-39
  - TASK-5.4
references:
  - docs/PLAN.md
priority: medium
ordinal: 65000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Phase 2 exit criterion, automated so it stays true.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A headless test builds a 20-clip, 3-track sequence with splits, trims and crossfades via the Command API
- [ ] #2 Undo all then redo all yields identical project JSON
- [ ] #3 Rendering frame 100 produces a non-black image
<!-- AC:END -->
