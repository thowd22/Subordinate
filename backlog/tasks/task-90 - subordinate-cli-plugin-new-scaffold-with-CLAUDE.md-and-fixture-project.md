---
id: TASK-90
title: subordinate-cli plugin new scaffold with CLAUDE.md and fixture project
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
  - cli
milestone: m-6
dependencies:
  - TASK-89
  - TASK-86
references:
  - docs/PLAN.md
priority: high
ordinal: 111000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Agent-first principle (§6.1): a plugin starts from a single command.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 plugin new --world <world> <name> generates a cargo-component project, plugin.toml, a CLAUDE.md describing the world interface, host imports and test contract, plus a fixture project
- [ ] #2 Generated project builds and installs with no edits
- [ ] #3 Templates exist for command, effect, analyzer and mcp-tools worlds
<!-- AC:END -->
