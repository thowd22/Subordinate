---
id: TASK-127
title: >-
  Wire the engine into SubordinateApp so panels apply commands instead of
  logging them
status: To Do
assignee: []
created_date: '2026-09-10 19:43'
labels:
  - ui
  - core
milestone: m-2
dependencies:
  - TASK-12
  - TASK-43
  - TASK-5.1
references:
  - docs/PLAN.md
priority: high
ordinal: 147000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Several merged UI tasks report the same gap: the assembled app shell (SubordinateApp in sub-ui) has no engine handle, so panel actions are logged rather than applied. TASK-111 notes insert/overwrite only logs the planned command, TASK-40 marker actions reach app.rs as log lines, TASK-71 says there is no file-open command or autosave worker instance because the app has no engine handle, and TASK-37's viewer half is blocked on the same wiring. Each panel is tested in isolation through the kittest harness, but the real app cannot edit a project. This is the glue that makes the UI functional and it must exist before end-to-end UI tests, the Xvfb smoke and any user trial mean anything.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 SubordinateApp owns an engine handle (sub-edit engine thread) and a Command API client; project open, save and autosave run through it
- [ ] #2 Timeline, bin, inspector, markers, track headers and sequence tabs dispatch their commands through the handle; the log-only placeholders in app.rs are removed
- [ ] #3 Engine change events refresh the panels (subscription wired) so an edit made via the MCP bridge appears in the running UI
- [ ] #4 An interaction test opens the sample project in the assembled app, splits a clip via the timeline, undoes it via the menu, and asserts project state through the Command API
- [ ] #5 The Xvfb window smoke shows the sample project loaded through the real engine path
<!-- AC:END -->
