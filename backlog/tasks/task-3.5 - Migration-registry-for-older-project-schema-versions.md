---
id: TASK-3.5
title: Migration registry for older project schema versions
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-3.4
references:
  - docs/PLAN.md
parent_task_id: TASK-3
priority: medium
ordinal: 23000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Old files must always open (§5.6). A registry pattern from version 1 onward avoids ad-hoc upgrade code later.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Migration trait with from_version, to_version and a function over serde_json::Value
- [ ] #2 Loader applies migrations in order and records the original version in a load report
- [ ] #3 A test fixture at schema_version 1 with a deliberately renamed field migrates and loads
<!-- AC:END -->
