---
id: TASK-7
title: >-
  Spike: decode H.264 via gstreamer-rs, upload NV12 to wgpu, show in egui with
  pop-out viewport
status: To Do
assignee: []
created_date: '2026-09-08 20:53'
labels:
  - spike
  - media
  - ui
milestone: m-1
dependencies:
  - TASK-1
references:
  - docs/PLAN.md
priority: high
ordinal: 7000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
This single spike de-risks both the preview phase and the second-display pop-out phase (PLAN.md §11 step 3). Hardware decode should be preferred where available. Throwaway code is acceptable; the findings are the deliverable.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A 4K H.264 file plays in an egui window on Linux using nvdec or va decode when available
- [ ] #2 The same frame texture is shown in a second egui viewport that can be moved to another monitor
- [ ] #3 Findings on frame upload cost and decode-to-display latency are written to backlog docs
<!-- AC:END -->
