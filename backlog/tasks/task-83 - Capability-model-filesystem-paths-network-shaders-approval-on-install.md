---
id: TASK-83
title: 'Capability model: filesystem paths, network, shaders, approval on install'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
  - security
milestone: m-6
dependencies:
  - TASK-82
references:
  - docs/PLAN.md
priority: high
ordinal: 104000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Plugins are sandboxed by default (§6.1).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Capabilities map to WASI preopens and host-function gating; $PROJECT and $PLUGIN_DATA path variables expand
- [ ] #2 Installing a plugin records approved capabilities; a changed manifest requires re-approval
- [ ] #3 A plugin without fs_read cannot open files (test)
<!-- AC:END -->
