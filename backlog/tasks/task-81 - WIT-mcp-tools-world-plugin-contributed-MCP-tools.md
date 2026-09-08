---
id: TASK-81
title: 'WIT mcp-tools world: plugin-contributed MCP tools'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
  - mcp
milestone: m-6
dependencies:
  - TASK-75
references:
  - docs/PLAN.md
priority: high
ordinal: 102000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Plugins extend the agent surface (§6.2, §7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 mcp-tools world exports tools() -> list<ToolDesc { name, description, json-schema }> and call(name, args-json) -> result-json
- [ ] #2 Manifest declares the tools; the host validates arguments against the schema before calling
- [ ] #3 Tools appear through the MCP bridge with the plugin id as a prefix
<!-- AC:END -->
