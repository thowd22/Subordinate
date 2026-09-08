---
id: TASK-87
title: Shader effect execution from plugin declarations in the compositor
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - render
  - plugins
milestone: m-6
dependencies:
  - TASK-76
  - TASK-84
  - TASK-39
references:
  - docs/PLAN.md
priority: high
ordinal: 108000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Connects the effect world to sub-render (§5.3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Compositor compiles the plugin's WGSL with a validated uniform layout from ParamDesc and caches by hash
- [ ] #2 Effects are applied per clip in order; a failing shader compile disables that effect with a visible error
- [ ] #3 Golden test: reference tint effect changes pixel values as expected
<!-- AC:END -->
