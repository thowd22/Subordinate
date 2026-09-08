---
id: TASK-91
title: subordinate-cli plugin test harness
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
  - cli
  - test
milestone: m-6
dependencies:
  - TASK-90
references:
  - docs/PLAN.md
priority: high
ordinal: 112000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Agents need a fast, headless way to prove a plugin works (§6.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 plugin test <id> loads the plugin headlessly, runs its declared tests against the fixture project and reports structured JSON results
- [ ] #2 Effect plugins are tested by rendering a frame; command plugins by asserting project state after run
- [ ] #3 Non-zero exit and readable output on failure
<!-- AC:END -->
