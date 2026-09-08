---
id: TASK-85
title: 'Plugin registry: install locations, enable/disable, listing'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-84
references:
  - docs/PLAN.md
priority: high
ordinal: 106000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users and agents need to see and manage installed plugins.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Plugins load from a user plugin dir and a project-local plugin dir; project-local wins on id conflict with a warning
- [ ] #2 plugin list, enable, disable, remove via CLI and MCP
- [ ] #3 Load failures are reported per plugin without aborting startup
<!-- AC:END -->
