---
id: TASK-38
title: 'Crossfade transition: model, command and render'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 08:07'
labels:
  - ui
  - render
milestone: m-2
dependencies:
  - TASK-21
  - TASK-31
references:
  - docs/PLAN.md
priority: medium
ordinal: 59000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The only MVP transition (§5.3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 AddTransition/RemoveTransition commands place a crossfade between adjacent clips with a duration clamped to available handles
- [ ] #2 Compositor blends the two clips linearly across the transition
- [ ] #3 Timeline draws the transition region and allows dragging its duration
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the crossfade's timeline drawing: a committed snapshot of a transition on the timeline, and an interaction test for however it is created or adjusted from the UI
<!-- AC:END -->
