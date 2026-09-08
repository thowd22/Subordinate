---
id: TASK-20
title: NV12 upload and YUV-to-RGB conversion shader
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - render
milestone: m-1
dependencies:
  - TASK-19
  - TASK-14
references:
  - docs/PLAN.md
priority: high
ordinal: 41000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decoded frames arrive as NV12; converting on the GPU is the fastest path until zero-copy import lands.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Two-plane texture upload for NV12 with a WGSL shader producing RGBA with Rec.709 limited-range conversion
- [ ] #2 Handles odd dimensions and stride padding
- [ ] #3 A golden test compares a converted colour-bar frame against reference pixel values within tolerance
<!-- AC:END -->
