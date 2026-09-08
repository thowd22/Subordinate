---
id: TASK-58
title: Full-resolution compositor readback path for export
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - render
  - export
milestone: m-4
dependencies:
  - TASK-39
references:
  - docs/PLAN.md
priority: high
ordinal: 79000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Export must never drop frames and needs frames on the CPU (or in GPU memory the encoder accepts).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 render_frame_to_buffer(sequence, time) returns an RGBA or NV12 buffer at sequence resolution using a staging buffer ring to overlap GPU and CPU work
- [ ] #2 Throughput on the 1080p fixture exceeds 60 fps on a discrete GPU (measured)
- [ ] #3 Readback never runs on the UI thread
<!-- AC:END -->
