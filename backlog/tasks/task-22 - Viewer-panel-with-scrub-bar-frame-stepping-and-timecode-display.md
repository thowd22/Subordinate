---
id: TASK-22
title: 'Viewer panel with scrub bar, frame stepping and timecode display'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - ui
milestone: m-1
dependencies:
  - TASK-21
  - TASK-18
references:
  - docs/PLAN.md
priority: high
ordinal: 43000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
First user-visible milestone: open a file and scrub it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Viewer shows the compositor texture scaled to fit with correct aspect
- [ ] #2 Scrub bar drag, left/right arrow frame step, Home/End work and display the timecode from sub-time
- [ ] #3 Scrubbing the 4K fixture on a hardware-decode Linux machine sustains above 30 fps (measured and recorded in the task notes)
<!-- AC:END -->
