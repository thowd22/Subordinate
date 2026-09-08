---
id: TASK-63
title: subordinate-cli render command
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - cli
  - export
milestone: m-4
dependencies:
  - TASK-60
  - TASK-61
  - TASK-6
references:
  - docs/PLAN.md
priority: high
ordinal: 84000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Headless render is the CI smoke test and what agents call (§5.5).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 subordinate-cli render project.sub --sequence NAME --preset NAME --out PATH renders with progress on stderr and exit code 0 on success
- [ ] #2 --encoder overrides selection; --range trims
- [ ] #3 CI renders the sample project with x264 on all three OSes and validates the output with the discoverer
<!-- AC:END -->
