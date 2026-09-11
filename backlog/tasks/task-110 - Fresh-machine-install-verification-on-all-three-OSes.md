---
id: TASK-110
title: Fresh-machine install verification on all three OSes
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 13:41'
labels:
  - release
  - verify
milestone: m-7
dependencies:
  - TASK-106
  - TASK-107
  - TASK-109
references:
  - docs/PLAN.md
priority: high
ordinal: 131000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Phase 7 exit criterion.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 On clean Ubuntu, Windows and macOS machines the package installs, opens the sample project, plays with audio and exports with the best available encoder
- [ ] #2 Each run is recorded with OS version, GPU and result in a backlog doc
- [ ] #3 Any failure becomes a blocking task before release
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor: macOS half of the fresh-install check is deferred with TASK-105/117; Linux and Windows proceed.
<!-- SECTION:NOTES:END -->
