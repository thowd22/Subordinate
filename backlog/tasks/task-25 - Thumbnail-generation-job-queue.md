---
id: TASK-25
title: Thumbnail generation job queue
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-15
  - TASK-12
references:
  - docs/PLAN.md
priority: medium
ordinal: 46000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Bins and timeline strips need thumbnails without blocking editing (§5.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A background job service with priorities, cancellation and progress events
- [ ] #2 Thumbnail job produces a strip of N frames per media item into the sidecar dir as compressed images
- [ ] #3 Jobs resume after restart and skip already-generated files
<!-- AC:END -->
