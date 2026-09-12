---
id: TASK-139
title: >-
  Desktop automation harness: drive the real app with input and screenshots,
  assert with vision or pixel checks
status: To Do
assignee: []
created_date: '2026-09-11 22:18'
updated_date: '2026-09-12 06:10'
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
- [ ] #6 Linux desktop flow compares the viewer's screenshot region against a frame rendered by subordinate-cli for the same playhead (moved from TASK-144 criterion 1)
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 user requirement: both the MCP path and real GUI clicking must be exercised on the desktop images; criteria split accordingly.
<!-- SECTION:NOTES:END -->
