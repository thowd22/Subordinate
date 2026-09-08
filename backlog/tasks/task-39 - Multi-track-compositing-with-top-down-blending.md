---
id: TASK-39
title: Multi-track compositing with top-down blending
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - render
milestone: m-2
dependencies:
  - TASK-21
  - TASK-34
references:
  - docs/PLAN.md
priority: high
ordinal: 60000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Stacked video tracks must composite in order with per-clip opacity (§5.3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Compositor iterates video tracks top-down, skipping muted tracks, blending with premultiplied alpha
- [ ] #2 Gaps are transparent; the bottom is black
- [ ] #3 A golden test with two overlapping colour-bar clips at 50% opacity matches reference
<!-- AC:END -->
