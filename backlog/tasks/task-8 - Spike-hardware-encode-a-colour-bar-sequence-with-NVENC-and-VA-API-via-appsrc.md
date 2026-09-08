---
id: TASK-8
title: 'Spike: hardware encode a colour-bar sequence with NVENC and VA-API via appsrc'
status: To Do
assignee: []
created_date: '2026-09-08 20:53'
labels:
  - spike
  - export
milestone: m-4
dependencies:
  - TASK-1
references:
  - docs/PLAN.md
priority: high
ordinal: 8000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GStreamer was chosen specifically for safe-Rust hardware encode (decision-1). Verify nvh264enc and vah264enc actually work end-to-end from an appsrc feed on real hardware before building the export crate on top.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An mp4 encoded with nvh264enc from appsrc plays correctly on an NVIDIA machine
- [ ] #2 An mp4 encoded with vah264enc from appsrc plays correctly on an AMD Linux machine
- [ ] #3 Encoder availability probe logic and observed caveats are written to backlog docs
<!-- AC:END -->
