---
id: TASK-93
title: 'subordinate-mcp: rmcp stdio bridge to the Command API'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - mcp
milestone: m-6
dependencies:
  - TASK-5.4
  - TASK-6
references:
  - docs/PLAN.md
priority: high
ordinal: 114000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The MCP server is how Claude Code drives the editor (§7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Binary speaks MCP over stdio using rmcp, connects to the running app's socket, or launches subordinate-cli serve when none is running
- [ ] #2 .mcp.json entry for Claude Code is documented and committed as an example
- [ ] #3 Tools are generated from docs/schema/command-api.json so names and descriptions match
<!-- AC:END -->
