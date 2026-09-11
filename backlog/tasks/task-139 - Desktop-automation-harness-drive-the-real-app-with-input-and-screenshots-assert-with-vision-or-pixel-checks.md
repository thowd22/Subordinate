---
id: TASK-139
title: >-
  Desktop automation harness: drive the real app with input and screenshots,
  assert with vision or pixel checks
status: To Do
assignee: []
created_date: '2026-09-11 22:18'
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
- [ ] #2 An end-to-end flow on each desktop runner imports the baked MP4 through the real Import dialog, drops it on the timeline, splits it, exports with the vendor encoder, and validates the file; screenshots at each step are uploaded
- [ ] #3 The flow runs nightly and on release tags via hardware.yml, costs under 0.30 USD per run on spot, and its failures name the step and attach the screenshot
<!-- AC:END -->
