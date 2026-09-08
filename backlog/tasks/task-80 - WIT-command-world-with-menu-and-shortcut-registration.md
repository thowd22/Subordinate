---
id: TASK-80
title: WIT command world with menu and shortcut registration
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
  - TASK-42
references:
  - docs/PLAN.md
priority: high
ordinal: 101000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Composite editing operations built from primitives (§6.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 command world exports commands() -> list<CommandDesc { id, title, shortcut? }> and run(id, context) that can call the host command-api
- [ ] #2 Host registers commands in a Plugins menu and the shortcut registry
- [ ] #3 A plugin command run is one undo group
<!-- AC:END -->
