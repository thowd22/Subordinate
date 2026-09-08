---
id: TASK-95
title: MCP resources and cached list results
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - mcp
milestone: m-6
dependencies:
  - TASK-93
references:
  - docs/PLAN.md
priority: medium
ordinal: 116000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Resources let agents read project state without tool calls.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Resources: project://current, sequence://{id}, media://{id} returning OTIO-shaped JSON
- [ ] #2 List results carry ttlMs per the 2026-07-28 spec; resource updates notify subscribers via the event bus
- [ ] #3 Tested with the MCP inspector
<!-- AC:END -->
