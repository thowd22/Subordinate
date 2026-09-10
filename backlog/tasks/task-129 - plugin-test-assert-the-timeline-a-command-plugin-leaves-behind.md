---
id: TASK-129
title: 'plugin test: assert the timeline a command plugin leaves behind'
status: To Do
assignee: []
created_date: '2026-09-10 21:58'
labels:
  - plugins
  - test
milestone: m-6
dependencies:
  - TASK-102
priority: medium
ordinal: 149000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the TASK-102 runbook run. The harness proves a command plugin changed the project and that one undo reversed it, but its project_state check carries only counts (clips, tracks, markers, revision), so a plugin that moved every clip to the wrong place passes exactly like one that got it right. In the recorded run the agent's first build removed 25 frames instead of 15 and the harness could not tell; only the plugin's own answer revealed it. A fixture needs a way to state the timeline it expects afterwards so plugin test checks the result rather than the fact of a change.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fixture can declare the track layout expected after a command plugin runs, and plugin test reports a failed check when the result differs
- [ ] #2 The expectation is exact RationalTime, not seconds
- [ ] #3 A fixture that declares no expectation behaves as it does today
<!-- AC:END -->
