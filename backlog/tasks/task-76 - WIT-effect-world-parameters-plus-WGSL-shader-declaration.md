---
id: TASK-76
title: 'WIT effect world: parameters plus WGSL shader declaration'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
references:
  - docs/PLAN.md
priority: high
ordinal: 97000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GPU effects are shaders declared by plugins and run by the core (decision-6).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 effect world exports describe() -> EffectDesc { params: list<ParamDesc>, shader: string, entry: string }
- [ ] #2 ParamDesc supports float, int, bool, color and enum with ranges and defaults
- [ ] #3 Optional process-cpu(frame, params) export for small-buffer processing is defined but marked slow in docs
<!-- AC:END -->
