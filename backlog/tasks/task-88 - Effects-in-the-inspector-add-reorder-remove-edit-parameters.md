---
id: TASK-88
title: 'Effects in the inspector: add, reorder, remove, edit parameters'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:07'
labels:
  - ui
  - plugins
milestone: m-6
dependencies:
  - TASK-87
  - TASK-37
references:
  - docs/PLAN.md
priority: medium
ordinal: 109000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users need to apply plugin effects without an agent.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Inspector lists applied effects with parameter widgets generated from ParamDesc
- [ ] #2 Add-effect picker lists installed effect plugins
- [ ] #3 All edits are undoable commands
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the effect list in the inspector: a committed snapshot with effects present, and interaction tests for add, reorder and remove asserting the commands issued
<!-- AC:END -->
