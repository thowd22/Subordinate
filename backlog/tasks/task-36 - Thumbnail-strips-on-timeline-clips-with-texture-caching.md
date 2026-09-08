---
id: TASK-36
title: Thumbnail strips on timeline clips with texture caching
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-28
  - TASK-25
references:
  - docs/PLAN.md
priority: medium
ordinal: 57000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Visual identification of clips on the timeline (§5.7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Video clips show thumbnail frames along their length, sampled from the thumbnail job output
- [ ] #2 Strips are cached as egui textures per zoom bucket and evicted under a budget
- [ ] #3 No visible stall when scrolling a 200-clip sequence
<!-- AC:END -->
