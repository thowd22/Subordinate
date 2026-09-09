---
id: TASK-120
title: Snapshot and interaction tests for the timeline panel
status: To Do
assignee: []
created_date: '2026-09-09 18:21'
labels:
  - ui
  - test
milestone: m-2
dependencies:
  - TASK-119
  - TASK-30
  - TASK-32
priority: high
ordinal: 140000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The timeline is the most complex custom-painted surface and the most likely to regress visually. Cover it with kittest snapshots at three zoom levels plus scripted interactions that assert model state through the engine, not pixels.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Snapshots: empty sequence, the sample project at fit-to-window zoom, and single-frame zoom around the playhead
- [ ] #2 Interaction tests: click selects a clip, drag moves it and produces one undo step, Ctrl+K splits at the playhead, S toggles snapping; each asserts project state via the Command API
- [ ] #3 Tests run in under 20 seconds total on the hosted Linux runner
<!-- AC:END -->
