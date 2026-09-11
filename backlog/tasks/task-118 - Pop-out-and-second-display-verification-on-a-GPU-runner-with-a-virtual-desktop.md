---
id: TASK-118
title: Pop-out and second-display verification on a GPU runner with a virtual desktop
status: In Progress
assignee:
  - '@opus-task-118'
created_date: '2026-09-09 17:29'
updated_date: '2026-09-11 15:05'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. scripts/ui-smoke.sh: add --gpu (do not force LIBGL_ALWAYS_SOFTWARE so the real adapter is used), probe window geometry with xwininfo -root -tree into windows.txt, and add --require-popout-on-head N which fails the run unless the pop-out window's origin lies inside that head's rectangle; keep the hosted CI behaviour unchanged by default.
2. .github/workflows/hardware.yml: new free job on box (self-hosted, amd-gpu) that builds the GUI debug binary, runs the script under Xvfb with one wide screen split into two RandR monitors (two outputs), requires the pop-out on head 1, appends the summary and uploads one PNG per output plus the logs as an artifact. The NVIDIA spot runner is deliberately left out: nothing here is NVDEC-specific and box is free.
3. docs/DEVELOPMENT.md: document the interactive RDP-with-two-monitors procedure for the future Windows GPU runner (mstsc /multimon, the RDP display settings, how to confirm two heads and drive the pop-out and fullscreen-on-monitor checks by hand).
4. Record the 4K H.264 hardware-decode scrub numbers already produced by hardware.yml (TASK-116 runs) in the task notes rather than re-measuring them.
5. Verify: push task/task-118, run the workflow, watch it, download the artifacts and look at both PNGs; iterate up to three times. Then finalize per the guide with run ids.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: scoped to GPU-only checks; free-tier UI tests live in the m-2 UI testing tasks.

2026-09-09: box (self-hosted, AMD APU, 16 cores, no desktop session, xvfb and xdotool installed) is a zero-cost place for the two-output virtual desktop test; prefer it over the NVIDIA spot runner for everything except NVDEC-specific checks.
<!-- SECTION:NOTES:END -->
