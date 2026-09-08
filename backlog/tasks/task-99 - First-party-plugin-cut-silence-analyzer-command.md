---
id: TASK-99
title: 'First-party plugin: cut silence (analyzer + command)'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
  - first-party
milestone: m-6
dependencies:
  - TASK-79
  - TASK-80
  - TASK-91
references:
  - docs/PLAN.md
priority: high
ordinal: 120000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The canonical agent-built plugin and the phase 6 exit test target.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Analyzer detects silent ranges below a threshold; command removes them from selected clips with ripple in one undo group
- [ ] #2 Ships in plugins/ built by CI and installable via plugin install
- [ ] #3 Its CLAUDE.md is the reference for the scaffold template
<!-- AC:END -->
