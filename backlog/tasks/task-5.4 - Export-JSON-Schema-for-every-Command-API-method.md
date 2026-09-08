---
id: TASK-5.4
title: Export JSON Schema for every Command API method
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
  - mcp
milestone: m-0
dependencies:
  - TASK-5.3
references:
  - docs/PLAN.md
parent_task_id: TASK-5
priority: medium
ordinal: 33000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The MCP bridge and plugin SDK generate their tool and binding definitions from this schema, so it must be produced by the code, not hand-written.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 subordinate-cli schema dumps a JSON document listing every method with params and result schemas (schemars)
- [ ] #2 A CI check fails if the committed docs/schema/command-api.json differs from the generated one
- [ ] #3 Each method carries a one-sentence description used verbatim as its MCP tool description
<!-- AC:END -->
