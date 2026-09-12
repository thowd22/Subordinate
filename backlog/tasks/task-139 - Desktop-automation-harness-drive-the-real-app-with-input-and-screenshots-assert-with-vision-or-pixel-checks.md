---
id: TASK-139
title: >-
  Desktop automation harness: drive the real app with input and screenshots,
  assert with vision or pixel checks
status: In Progress
assignee:
  - '@opus-task-139'
created_date: '2026-09-11 22:18'
updated_date: '2026-09-12 15:32'
labels:
  - ui
  - test
  - plugins
milestone: m-8
dependencies:
  - TASK-137
  - TASK-138
priority: high
ordinal: 159000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A small library agents use on the desktop runners: launch the app with a project, send input (xdotool on Linux, UI Automation or pywinauto on Windows), wait for a window title or a log line, capture and annotate screenshots, and hand them back as artifacts and job summaries an agent can read. Pair it with the MCP bridge so a test can act semantically (timeline.add_clip) and verify visually, or act like a human (click Import, choose the baked MP4) and verify through the Command API state. Keep kittest snapshots for regressions; this harness is for end-to-end flows.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 scripts/desktop/ provides the same command set on Linux and Windows: launch, click, key, drag, wait_for_title, wait_for_log, screenshot, with documentation and an example
- [x] #2 The flow runs nightly and on release tags via hardware.yml, costs under 0.30 USD per run on spot, and its failures name the step and attach the screenshot
- [ ] #3 MCP flow on each desktop runner: an agent-style script drives subordinate-mcp only (project.new/open, media import of the baked MKV, timeline.add_clip, timeline split, export.render with the vendor encoder) while the real window is visible; a screenshot after each step is uploaded and the final export validates
- [ ] #4 Human flow on each desktop runner: input only (click the bin's Import button, choose the baked MKV in the native file dialog, drag the clip onto the timeline, press Ctrl+K at a scrubbed position, click Export in the panel, choose the vendor encoder); project state is asserted through the Command API after each step and screenshots are uploaded; no MCP calls are used to act
- [ ] #5 Both flows are separate jobs in hardware.yml, run nightly and on release tags, and a failure names the step and attaches its screenshot
- [ ] #6 Linux desktop flow compares the viewer's screenshot region against a frame rendered by subordinate-cli for the same playhead (moved from TASK-144 criterion 1)
- [x] #5 Both flows are separate jobs in hardware.yml, run nightly and on release tags, and a failure names the step and attaches its screenshot
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. scripts/desktop/: a Python package (subdesktop) with one command set on both OSes - launch, click, key, drag, wait_for_title, wait_for_log, screenshot, plus find/dump - over two backends: X11 (xdotool, scrot, xwininfo, AT-SPI names when the a11y bus is up) and Windows (pywinauto UI Automation, .NET screen capture). A module CLI (python -m subdesktop ...) so shell and PowerShell steps call the same verbs, with README.md documenting each command and a worked example.
2. Two flow scripts on top of it, run on both images: flow_mcp.py drives subordinate-mcp only (project.open, media.import of the baked MKV, timeline.add_clip, timeline.split_clip, then export.render with the vendor encoder) and screenshots after every step; flow_clicks.py uses input only (Import button by AccessKit name, native file dialog, drag to the timeline, Ctrl+K at a scrubbed position, Export panel with the vendor encoder pinned) and asserts project state through read-only Command API calls after every step. Both name their steps, write result.json and emit ::error:: naming the failing step and its screenshot.
3. Windows runs both flows inside the auto-logon console session through InteractiveSession.psm1 (the job is SYSTEM in session 0); Linux runs them directly with DISPLAY=:0.
4. .github/workflows/desktop-flows.yml holds the four jobs (linux-mcp, linux-clicks, windows-mcp, windows-clicks), chained so at most two g4dn instances run at once (the account quota), called from hardware.yml, which gains a release-tag trigger beside its nightly schedule. Screenshots and logs upload as artifacts; per-job cost budgeted under 0.30 USD.
5. Docs: a Desktop flows section in docs/DEVELOPMENT.md and scripts/desktop/README.md.
6. Verify by real dispatches from task/task-139 with the inline ami= runner selectors (named runners resolve from main only), record the run ids in the notes, and check only the criteria a green run proves.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 user requirement: both the MCP path and real GUI clicking must be exercised on the desktop images; criteria split accordingly.

Delivered on branch task/task-139.

scripts/desktop/ is a Python harness (subdesktop) with one command set on both desktop images - launch, click, key, drag, wait_for_title, wait_for_log, screenshot, plus find/controls and a module CLI - over an X11 backend (xdotool, scrot, AT-SPI) and a Windows backend (pywinauto UI Automation, .NET capture), with README.md, example.py and two flows: flow_mcp.py (acts only through subordinate-mcp) and flow_clicks.py (acts only through input, asserts through read-only Command API calls). .github/workflows/desktop-flows.yml runs the four jobs, two GPU instances at a time for the account quota, and hardware.yml calls it and now also runs on release tags.

What the runners taught, all of it now in docs/DEVELOPMENT.md:
- The window is on screen before its Command API socket is (run 34671892159).
- An agent's edit does not wake an idle egui window, so a screenshot taken straight after one shows the project as it was (run 34672010010); every MCP-flow picture is taken after a pointer nudge.
- The editor's endpoint does not serve export.*: only subordinate-cli serve installs the host-backed families, so the MCP flow saves and renders through a second bridge.
- The Linux desktop image's pointer cannot reach the whole screen - xdotool stops at x=448 on a 1920-wide screen - which is why clicks appeared to be ignored; the harness measures the reachable rectangle and maximizes into it (runs 34681396173, 34681751826).
- AccessKit publishes nothing on Linux until org.a11y.Status says accessibility is enabled; the harness sets it (run 34673633121).
- The dock's tabs are painted, not published, so the export panel's tab is found relative to the Inspector's own text.
- PACKAGING DEFECT: the v0.1.2 AppImage's libav, va, qsv and msdk plugins fail to load without libva-drm.so.2 and libvdpau.so.1, so media.probe of the baked 4K60 clip answers media.unsupported/MissingPlugins with no window involved (run 34684471869). The clicks job installs the libraries until the package carries them.
- WINDOWS DEFECT: an export of the baked 4K60 clip stalls after one frame on the Windows desktop runner - editor open or closed, through the bridge or through the GUI's Export button, with the Direct3D decoders ranked out in favour of NVDEC (runs 34672165182, 34673633121, 34674794109, 34682198476). The same project renders in seconds on Linux.

Verification, all on branch task/task-139 with the inline ami= runner selectors.

Final run 34685619604 (all four jobs):
- Linux desktop, MCP-only flow: PASSED in 3m. project.new/open, media.import of the baked MKV, timeline.add_clip, timeline.split_clip (2 clips read back), project.save, export.render -> completed, 96 frames, 21 MB, probed H.264, editor log names nvh264enc. A screenshot after every step.
- Linux desktop, clicks-only flow: PASSED in 3m08s. Mute probe, Import... clicked by AccessKit name, the GTK chooser's own row double-clicked, the clip dragged onto V1, Ctrl+K leaving 2 clips, the export panel opened, In/Out set to 120 frames, the path typed, nvh264enc pinned, Export clicked, file written and probed as H.264 with the editor's log naming nvh264enc, and history.get showing 'Split clip' over 5 undo steps. No mutating MCP call is reachable from that flow: ReadOnly refuses anything but a read.
- Windows desktop, MCP-only flow: every step through project.save and 'the window still holds what the agent edited' PASSED; the export stalls (see below).
- Windows desktop, clicks-only flow: every gesture PASSED - the mute probe, Import..., the native #32770 dialog, the drag onto V1, Ctrl+K leaving 2 clips, the export panel, the 120-frame range, the output path, the nvh264enc pin - and the Export click starts a real pipeline on nvh264enc (the editor's log names it) that then stalls.

Earlier runs used for the findings: 34671892159, 34672010010, 34672165182, 34673633121, 34674794109, 34676051424, 34676433658, 34676801744, 34677234822, 34677608304, 34677987056, 34678351781, 34678850753, 34679600299, 34680515589, 34681396173, 34681751826, 34682198476, 34682903107, 34683546727, 34684189192, 34684471869, 34684749594, 34685062551, 34685368578.

Cost: a Linux flow is 3-4 minutes of g4dn.xlarge (about 0.03 USD on spot, 0.08 USD at the 20-minute cap); a Windows flow is 8-11 minutes on demand (0.10-0.14 USD, 0.27 USD at the 22-minute cap). All four together are about 0.30 USD a night. Total spent iterating: roughly 3 USD.

AC 3 and AC 4 are checked for Linux and NOT for Windows: both Windows flows do everything the criteria ask except produce the file, because rendering the baked 4K60 clip on that runner stalls after one frame. That is not the harness - subordinate-cli render reproduces it in session 0 with no window and no bridge (the 'Render the same project with the CLI' step of run 34685619604, '1/48 frames 2% 1 fps' and then nothing) - so the criteria are left unchecked and the defect is handed over.

Two commits land after run 34685619604 and touch no flow logic: the temporary dispatcher workflow and the temporary Linux input-diagnosis step and script are removed (the finding they produced - the pointer's reachable band - is handled in the harness and documented). The probe-the-clip step is kept: ten seconds, and it tells a packaging regression apart from a broken gesture.

2026-09-12 supervisor handoff: Linux MCP and clicks flows pass end to end (run 34685619604); Windows flows pass every gesture but the export stalls (TASK-146). Remaining criteria: 3 and 4 on Windows (after TASK-146 and a Windows image rebuilt from the next release), and 6 (viewer screenshot versus a CLI-rendered frame, not built). Merged to main; nightly via hardware.yml.
<!-- SECTION:NOTES:END -->

## Comments

<!-- COMMENTS:BEGIN -->
author: @opus-task-139
created: 2026-09-12 10:00
---
AC 3 and AC 4 are complete on Linux and blocked on Windows by a defect this task found rather than by the flows: rendering the baked 4K60 clip on the Windows desktop runner stalls after one frame, reproduced with subordinate-cli render in session 0 with no window and no bridge (run 34685619604). Two decisions for the supervisor: whether to open a follow-up for that stall and for the AppImage's unloadable libav/va/qsv/msdk plugins (libva-drm.so.2, libvdpau.so.1), and whether to bake at-spi2-core, python3-pyatspi, xdg-desktop-portal(-gtk), zenity and those libraries into infra/images/linux-desktop so the clicks job stops installing them.
---
<!-- COMMENTS:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
scripts/desktop/ gives both desktop images the same seven verbs - launch, click, key, drag, wait_for_title, wait_for_log, screenshot - over X11 (xdotool, scrot, AT-SPI) and UI Automation (pywinauto, .NET), addressing controls by the AccessKit names egui publishes, with a README, a module CLI and a worked example. On top of it, flow_mcp.py drives subordinate-mcp and nothing else against the window on screen, and flow_clicks.py drives real input and asserts the result through read-only Command API calls; both photograph every step, write result.json and summary.md, and annotate a failure with the step's name and the file name of its screenshot. .github/workflows/desktop-flows.yml runs the four jobs, at most two GPU instances at a time for the account quota, and hardware.yml calls it nightly and on release tags.

Verified by real dispatches from task/task-139; final run 34685619604. Both Linux flows pass end to end, including an export the discoverer reads back as H.264 from nvh264enc. Both Windows flows pass every edit, gesture and assertion and fail only at their export, which stalls after one frame on that runner - reproduced with subordinate-cli render in session 0, with no window and no bridge - so AC 3 and AC 4 are left unchecked for Windows and the defect is written up in docs/DEVELOPMENT.md.

The work also found, and documented: the editor's window is on screen before its Command API socket is; an agent's edit does not wake an idle egui window; the editor's endpoint does not serve export.*; the Linux image's pointer cannot reach the left of its own screen; AccessKit publishes nothing until org.a11y.Status says accessibility is enabled; the dock's tabs are painted rather than published; and the AppImage's libav, va, qsv and msdk plugins fail to load for want of libva-drm.so.2 and libvdpau.so.1, which is why importing the baked clip answered media.unsupported.
<!-- SECTION:FINAL_SUMMARY:END -->
