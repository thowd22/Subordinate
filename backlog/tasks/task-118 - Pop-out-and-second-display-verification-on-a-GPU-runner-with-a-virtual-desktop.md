---
id: TASK-118
title: Pop-out and second-display verification on a GPU runner with a virtual desktop
status: Done
assignee:
  - '@opus-task-118'
created_date: '2026-09-09 17:29'
updated_date: '2026-09-11 15:59'
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
- [x] #1 A hardware.yml job starts a two-output virtual display on the NVIDIA Linux runner and runs the app's pop-out smoke test, capturing a screenshot of each output as an artifact
- [x] #2 Procedure for an interactive RDP session with two monitors on the Windows GPU runner is documented in docs/DEVELOPMENT.md
- [x] #3 The job also records 4K H.264 scrub fps with hardware decode from the benchmark harness in the job summary, closing TASK-22 criterion 3 and TASK-7 criterion 1 when met
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

2026-09-11 (opus-task-118): implemented and verified on box.

Changes:
- scripts/ui-smoke.sh: new --gpu (ask for the machine's real adapter) and --require-popout-on-head N (read the pop-out window's absolute geometry back with xwininfo -root -tree and fail unless it is inside that head's rectangle with the editor on another head); tries xrandr --setmonitor in three spellings, each verified against the server's own monitor list; writes windows.txt and monitors.txt beside the screenshots; outlines and names each window on its own head's PNG.
- .github/workflows/hardware.yml: new free job popout-two-output-linux on box (self-hosted, amd-gpu) - tooling and Vulkan-ICD preflight, installs x11-xserver-utils if xrandr is missing, builds subordinate (debug), runs the smoke on a 2560x800 Xvfb split into two 1280x800 heads with the pop-out required on head 1, asserts one screenshot per output, writes the job summary and uploads popout-two-output-<sha>. The NVIDIA spot runner is deliberately not used: nothing here is NVDEC-specific and box is free.
- .github/workflows/ci.yml: the hosted ui-smoke artifact now also carries windows.txt and monitors.txt.
- docs/DEVELOPMENT.md: the new job, the two virtual-display limits below, and a full manual RDP-with-two-monitors procedure for the future Windows GPU runner.

Verification: nine runs from task/task-118 behind a temporary push trigger (the three existing jobs guarded off so no GPU spot instance ever started; trigger and guards removed in the final commit, so workflow_dispatch for this job is unproven until it lands on main, exactly as TASK-116 found). Run ids: 34614583394, 34614668762, 34615366090, 34616222671, 34616775501, 34617247142 (first green), 34617711300, 34618215215, 34618654738 (final, green, annotated screenshots). Final artifact popout-two-output-3eac4c3: screen-0.png (1280x800, the editor window outlined and named, dock and timeline painted, viewer saying 'Showing in the pop-out window'), screen-1.png (1280x800, the pop-out outlined and named at 960x540 on head 1), app.log, app-gpu-attempt.log, windows.txt, monitors.txt, screens.txt, summary.md.

Two findings about virtual X displays, both recorded in docs/DEVELOPMENT.md and in the artifact rather than papered over:
1. Xvfb has no DRI3, so a real GPU cannot present into it: RADV was chosen and then refused the surface ('There was no valid format for the surface at all') before the first frame (run 34614668762). --gpu therefore means 'use the real adapter if this display can present it'; the script says so and redraws on lavapipe. LIBGL_ALWAYS_SOFTWARE is a GL variable and did nothing - the fallback points VK_DRIVER_FILES at lvp_icd.json. Real-GPU rendering of the compositor is proved headlessly by the amd-linux and nvidia-linux jobs; this job proves the windows.
2. Ubuntu 26.04's Xvfb reports RandR 1.6, accepts RRSetMonitor without an X error, and creates no monitor - three spellings, no complaint, one automatic whole-screen monitor afterwards (runs 34615366090, 34616222671). The two outputs are therefore two rectangles of one virtual screen. Window placement is unaffected (X windows are positioned in root coordinates and the pop-out lands and is photographed where it is put), but the app's own display enumeration sees one monitor there, so TASK-68's monitor picker cannot be exercised on this runner and TASK-68 AC 3 stays open for the RDP procedure.

Smaller things learned: box had no xrandr (the job installs x11-xserver-utils when missing) and has no ImageMagick font, so the label is drawn only when a TrueType file is found and the outlining never fails the run; both windows paint on black on a black root, which is why the outlines exist at all.

AC 3: the job summary records the 4K H.264 hardware-decode scrub the sibling jobs already measured on run 34612380072 - 8.256 fps through nvh264dec on the Tesla T4 and 9.131 fps through vah264dec on box - rather than measuring them again. Both are far below the 30 fps TASK-22 AC 3 and TASK-7 AC 1 ask for, so those criteria are not closed by this; the gap is TASK-133.
<!-- SECTION:NOTES:END -->

## Comments

<!-- COMMENTS:BEGIN -->
author: @opus-task-118
created: 2026-09-11 15:59
---
AC 1 is checked on box rather than on the NVIDIA Linux runner, per the supervisor's note on this task: nothing in it is NVDEC-specific and box is free. Two qualifications the supervisor should see: the 'two outputs' are two rectangles of one Xvfb screen, because Ubuntu 26.04's Xvfb accepts xrandr --setmonitor and creates nothing, so the app's own monitor enumeration still sees one display there (TASK-68's picker remains unproven, and the RDP procedure is what covers it); and the windows were drawn by lavapipe, not RADV, because Xvfb has no DRI3 and Mesa refuses the surface - real-GPU rendering stays proved by the headless render and readback jobs. If either qualification matters, the next step is a headless wlroots compositor on box (sway is not installed there), which brings its own problem: Wayland clients cannot position their own windows, so the pop-out would have to be moved with swaymsg.
---
<!-- COMMENTS:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
hardware.yml gains popout-two-output-linux, a free job on box that starts a two-output virtual display, launches the assembled editor with the sample project and its pop-out viewer on the second output, asserts from the server's own window geometry that the pop-out landed there with the editor elsewhere, and uploads one annotated screenshot per output. scripts/ui-smoke.sh gained --gpu and --require-popout-on-head N for it, plus windows.txt and monitors.txt. Verified by run https://github.com/thowd22/Subordinate/actions/runs/34618654738 (artifact popout-two-output-3eac4c3). docs/DEVELOPMENT.md documents the job, the manual RDP-with-two-monitors procedure for the Windows GPU runner, and two limits found along the way: Xvfb has no DRI3 so a real GPU cannot present into it (the run falls back to lavapipe and says so), and Ubuntu 26.04's Xvfb silently ignores xrandr --setmonitor, so the app's own monitor picker still needs a real second head.
<!-- SECTION:FINAL_SUMMARY:END -->
