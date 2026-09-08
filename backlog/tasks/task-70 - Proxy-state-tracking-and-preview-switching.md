---
id: TASK-70
title: Proxy state tracking and preview switching
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - media
  - ui
milestone: m-5
dependencies:
  - TASK-69
  - TASK-59
references:
  - docs/PLAN.md
priority: high
ordinal: 91000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Preview uses proxies, export never does.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 MediaItem proxy state (none, generating, ready, stale) is shown in the bin
- [ ] #2 Viewer toggle uses proxies when ready; export pipeline always uses originals (test asserts)
- [ ] #3 Proxies are invalidated when the source content hash changes
<!-- AC:END -->
