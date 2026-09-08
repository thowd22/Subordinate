---
id: TASK-19
title: wgpu device and eframe integration with a shared render context
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - render
  - ui
milestone: m-1
dependencies:
  - TASK-1.1
references:
  - docs/PLAN.md
priority: high
ordinal: 40000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The compositor and UI share one wgpu device so preview textures need no copies (§3 GUI decision).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 eframe launches with the wgpu backend and exposes device, queue and adapter info to sub-render
- [ ] #2 Adapter selection prefers a discrete GPU and logs the backend (Vulkan, D3D12, Metal)
- [ ] #3 App starts and renders an empty window on all three OSes in CI using a software adapter where needed
<!-- AC:END -->
