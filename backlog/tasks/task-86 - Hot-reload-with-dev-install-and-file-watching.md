---
id: TASK-86
title: Hot reload with --dev install and file watching
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-85
references:
  - docs/PLAN.md
priority: high
ordinal: 107000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The agent development loop depends on fast reload (§6.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 plugin install --dev symlinks or watches the built .wasm and reloads within a second of change
- [ ] #2 Reload preserves engine state and re-registers commands, effects and tools
- [ ] #3 Reload errors are shown in a plugin panel and returned to the CLI/MCP caller
<!-- AC:END -->
