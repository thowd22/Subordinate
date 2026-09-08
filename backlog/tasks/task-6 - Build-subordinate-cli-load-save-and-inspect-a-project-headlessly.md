---
id: TASK-6
title: 'Build subordinate-cli: load, save and inspect a project headlessly'
status: To Do
assignee: []
created_date: '2026-09-08 20:53'
labels:
  - cli
milestone: m-0
dependencies:
  - TASK-5
references:
  - docs/PLAN.md
priority: medium
ordinal: 6000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A headless CLI is the CI smoke test and the fallback the MCP bridge launches when no GUI is running (PLAN.md §7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 subordinate-cli new, open, save and inspect commands work on a sample project
- [ ] #2 subordinate-cli serve exposes the Command API socket without a GUI
<!-- AC:END -->
