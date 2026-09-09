---
id: TASK-118
title: Pop-out and second-display verification on a GPU runner with a virtual desktop
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
updated_date: '2026-09-09 21:35'
labels:
  - infra
  - gpu
  - ui
milestone: m-8
dependencies:
  - TASK-123
priority: medium
ordinal: 138000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The decode spike and the pop-out viewer have criteria that need a real GPU and a desktop with two displays. Free-tier UI tests (kittest snapshots and the Xvfb smoke job) cover layout and behaviour; this job covers only what needs the NVIDIA driver: real-GPU rendering of the assembled app, hardware-decoded playback in the viewer, and the pop-out on two virtual outputs. Cost rules: runs on gpu-nvidia-linux spot only, on workflow_dispatch and at most one nightly run, timeout-minutes 15, no Windows job (RDP checks stay manual). Expected cost under 0.05 USD per run.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A hardware.yml job starts a two-output virtual display on the NVIDIA Linux runner and runs the app's pop-out smoke test, capturing a screenshot of each output as an artifact
- [ ] #2 Procedure for an interactive RDP session with two monitors on the Windows GPU runner is documented in docs/DEVELOPMENT.md
- [ ] #3 The job also records 4K H.264 scrub fps with hardware decode from the benchmark harness in the job summary, closing TASK-22 criterion 3 and TASK-7 criterion 1 when met
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: scoped to GPU-only checks; free-tier UI tests live in the m-2 UI testing tasks.

2026-09-09: box (self-hosted, AMD APU, 16 cores, no desktop session, xvfb and xdotool installed) is a zero-cost place for the two-output virtual desktop test; prefer it over the NVIDIA spot runner for everything except NVDEC-specific checks.
<!-- SECTION:NOTES:END -->
