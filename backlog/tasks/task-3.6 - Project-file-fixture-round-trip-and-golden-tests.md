---
id: TASK-3.6
title: Project file fixture round-trip and golden tests
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
  - test
milestone: m-0
dependencies:
  - TASK-3.5
references:
  - docs/PLAN.md
parent_task_id: TASK-3
priority: medium
ordinal: 24000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Byte-identical round-trips prove the format is stable and give later refactors a regression net.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A committed sample project JSON with two sequences, three tracks each, clips, a crossfade, markers and two bins loads and saves byte-identically
- [ ] #2 Golden test fails with a readable diff when output changes
- [ ] #3 Sidecar directory naming (project.sub.d/) and its gitignore entry are documented in docs/PLAN.md §5.6 and DEVELOPMENT.md
<!-- AC:END -->
