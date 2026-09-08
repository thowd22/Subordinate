---
id: TASK-72
title: Relink offline media by content hash and folder search
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - ui
  - core
milestone: m-5
dependencies:
  - TASK-35
  - TASK-3.3
references:
  - docs/PLAN.md
priority: medium
ordinal: 93000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Projects move; relinking must be quick and safe.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Relink dialog lets the user pick a file or a folder to search recursively; matches by content hash first, then by name
- [ ] #2 Bulk relink applies to all offline items and is one undo step
- [ ] #3 A relinked item updates its relative path and clears the offline flag
<!-- AC:END -->
