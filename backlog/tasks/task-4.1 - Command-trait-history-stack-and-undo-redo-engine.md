---
id: TASK-4.1
title: 'Command trait, history stack and undo/redo engine'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-3.6
references:
  - docs/PLAN.md
parent_task_id: TASK-4
priority: high
ordinal: 25000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every mutation must be an undoable command because the same set is exposed to MCP and plugins (decision-7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Command trait with apply(&mut Project) -> Result<Inverse> and a History with undo, redo, clear and a configurable depth
- [ ] #2 Commands are serde-serialisable so they can be sent over the Command API and logged
- [ ] #3 Grouping API lets several commands undo as one step (needed for drags)
- [ ] #4 Tests prove undo then redo yields JSON-identical project state
<!-- AC:END -->
