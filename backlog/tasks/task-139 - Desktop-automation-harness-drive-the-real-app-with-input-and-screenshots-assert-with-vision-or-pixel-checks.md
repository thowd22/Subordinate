---
id: TASK-139
title: >-
  Desktop automation harness: drive the real app with input and screenshots,
  assert with vision or pixel checks
status: In Progress
assignee:
  - '@opus-task-139'
created_date: '2026-09-11 22:18'
updated_date: '2026-09-12 03:50'
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
- [ ] #1 scripts/desktop/ provides the same command set on Linux and Windows: launch, click, key, drag, wait_for_title, wait_for_log, screenshot, with documentation and an example
- [ ] #2 The flow runs nightly and on release tags via hardware.yml, costs under 0.30 USD per run on spot, and its failures name the step and attach the screenshot
- [ ] #3 MCP flow on each desktop runner: an agent-style script drives subordinate-mcp only (project.new/open, media import of the baked MKV, timeline.add_clip, timeline split, export.render with the vendor encoder) while the real window is visible; a screenshot after each step is uploaded and the final export validates
- [ ] #4 Human flow on each desktop runner: input only (click the bin's Import button, choose the baked MKV in the native file dialog, drag the clip onto the timeline, press Ctrl+K at a scrubbed position, click Export in the panel, choose the vendor encoder); project state is asserted through the Command API after each step and screenshots are uploaded; no MCP calls are used to act
- [ ] #5 Both flows are separate jobs in hardware.yml, run nightly and on release tags, and a failure names the step and attaches its screenshot
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
<!-- SECTION:NOTES:END -->
