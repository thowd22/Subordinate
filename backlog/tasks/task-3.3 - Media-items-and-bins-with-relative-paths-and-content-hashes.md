---
id: TASK-3.3
title: Media items and bins with relative paths and content hashes
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
ordinal: 21000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Projects move between machines. Relative paths plus a content hash let media be relinked reliably (§5.6).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 MediaItem stores project-relative path, content hash (BLAKE3 of first and last 1 MiB plus size), probed stream info, proxy state, offline flag
- [ ] #2 Bins form a tree with a root bin; items reference bins by ID
- [ ] #3 Helper resolves absolute paths from the project location and reports offline items
<!-- AC:END -->
