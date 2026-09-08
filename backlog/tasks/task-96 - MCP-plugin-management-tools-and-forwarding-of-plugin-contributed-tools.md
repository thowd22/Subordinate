---
id: TASK-96
title: MCP plugin management tools and forwarding of plugin-contributed tools
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - mcp
  - plugins
milestone: m-6
dependencies:
  - TASK-93
  - TASK-91
  - TASK-81
references:
  - docs/PLAN.md
priority: high
ordinal: 117000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Closes the agent loop: the agent installs and tests its own plugin from the conversation (§6.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 plugin.new, plugin.install, plugin.reload, plugin.test, plugin.list exposed as MCP tools
- [ ] #2 Tools from mcp-tools plugins are listed with the plugin id prefix and list_changed is sent on reload
- [ ] #3 Integration test installs a plugin and calls its tool through MCP
<!-- AC:END -->
