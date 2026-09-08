---
id: TASK-3.4
title: JSON serialisation with schema_version and stable field ordering
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-3.2
  - TASK-3.3
references:
  - docs/PLAN.md
parent_task_id: TASK-3
priority: high
ordinal: 22000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The project file must be diffable in git (§3), so key order and formatting must be deterministic.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Project serialises to pretty JSON with sorted map keys and a top-level schema_version integer
- [ ] #2 Loading rejects unknown schema_version above current with a clear SubError
- [ ] #3 A JSON Schema for the project file is generated (schemars) and committed under docs/schema/
<!-- AC:END -->
