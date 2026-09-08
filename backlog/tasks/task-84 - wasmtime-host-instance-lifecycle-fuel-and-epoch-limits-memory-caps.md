---
id: TASK-84
title: 'wasmtime host: instance lifecycle, fuel and epoch limits, memory caps'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-83
references:
  - docs/PLAN.md
priority: high
ordinal: 105000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A bad plugin must not stall or crash the engine (§4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Instances are created per plugin with configured memory limit, epoch-based interruption and fuel for CPU-bound calls
- [ ] #2 A deliberately infinite loop plugin is terminated and reported without affecting other plugins
- [ ] #3 Instance pooling keeps warm instances for hot paths like command and effect describe
<!-- AC:END -->
