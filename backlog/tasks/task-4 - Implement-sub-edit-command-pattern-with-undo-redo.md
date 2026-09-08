---
id: TASK-4
title: 'Implement sub-edit: command pattern with undo/redo'
status: To Do
assignee: []
created_date: '2026-09-08 20:53'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-3
references:
  - docs/PLAN.md
priority: high
ordinal: 4000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every mutation of project state is a Command with apply and revert. This same command set is what the Command API and the MCP server expose, so no editing logic may bypass it (PLAN.md §5.1, decision-7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Commands exist for add/remove/move/trim/split clip, add/remove track, add/remove sequence, set clip params
- [ ] #2 Undo and redo stacks restore exact prior state, verified by JSON equality in tests
- [ ] #3 Commands validate inputs and return structured errors rather than panicking
<!-- AC:END -->
