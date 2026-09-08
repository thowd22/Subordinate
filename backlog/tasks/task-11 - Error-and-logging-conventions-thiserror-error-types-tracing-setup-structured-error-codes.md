---
id: TASK-11
title: >-
  Error and logging conventions: thiserror error types, tracing setup,
  structured error codes
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-1.1
references:
  - docs/PLAN.md
priority: medium
ordinal: 15000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Errors must be machine-readable because plugins and the MCP bridge surface them to agents (§6.4). Deciding on one convention before the model and command crates are written avoids retrofitting.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A shared SubError type with a stable string code, human message and optional details map, serializable to JSON
- [ ] #2 tracing is initialised in every binary with env-filter and a JSON output option
- [ ] #3 docs/DEVELOPMENT.md documents the convention: when to add a code, how to wrap lower-level errors
<!-- AC:END -->
