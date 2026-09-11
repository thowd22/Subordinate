---
id: TASK-8
title: 'Spike: hardware encode a colour-bar sequence with NVENC and VA-API via appsrc'
status: Done
assignee: []
created_date: '2026-09-08 20:53'
updated_date: '2026-09-11 15:03'
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
- [x] #1 An mp4 encoded with nvh264enc from appsrc plays correctly on an NVIDIA machine
- [x] #2 An mp4 encoded with vah264enc from appsrc plays correctly on an AMD Linux machine
- [x] #3 Encoder availability probe logic and observed caveats are written to backlog docs
<!-- AC:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Superseded by TASK-116: the hardware workflow renders the sample project through appsrc-fed nvh264enc on a T4 and vah264enc on the box APU, validated by the discoverer, with encoder probe logic and caveats recorded in docs/DEVELOPMENT.md and the job summaries (run https://github.com/thowd22/Subordinate/actions/runs/34612380072).
<!-- SECTION:FINAL_SUMMARY:END -->
