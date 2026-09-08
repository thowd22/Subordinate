---
id: TASK-5.3
title: Event subscription over the Command API
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
  - mcp
milestone: m-0
dependencies:
  - TASK-5.2
references:
  - docs/PLAN.md
parent_task_id: TASK-5
priority: medium
ordinal: 32000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
MCP clients and pop-out windows need to know when the project changes without polling.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 events.subscribe returns a subscription ID and streams ChangeEvent notifications; events.unsubscribe stops them
- [ ] #2 Slow subscribers are dropped after a bounded backlog with a logged warning rather than stalling the engine
- [ ] #3 Test proves a second client sees a change made by the first
<!-- AC:END -->
