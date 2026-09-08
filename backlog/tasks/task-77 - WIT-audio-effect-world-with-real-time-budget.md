---
id: TASK-77
title: WIT audio-effect world with real-time budget
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
priority: medium
ordinal: 98000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Simple audio processing in plugins (§6.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 audio-effect world exports describe() and process(block: list<f32>, channels, rate, params) -> list<f32>
- [ ] #2 Host enforces a per-block time budget and bypasses the plugin after repeated overruns
- [ ] #3 Reference plugin: gain with a smoothing parameter
<!-- AC:END -->
