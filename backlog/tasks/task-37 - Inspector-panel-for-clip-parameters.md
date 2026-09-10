---
id: TASK-37
title: Inspector panel for clip parameters
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 08:07'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-30
  - TASK-21
references:
  - docs/PLAN.md
priority: medium
ordinal: 58000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Opacity and transform edits need a UI that maps directly to SetClipParams.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Selecting a clip shows opacity, position, scale, rotation, gain and fade fields
- [ ] #2 Edits apply live to the viewer and commit one undoable command on release
- [ ] #3 Multi-selection edits apply to all selected clips
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the inspector: a committed snapshot of the panel over the sample project, and an interaction test that edits a parameter and asserts the command it issues
<!-- AC:END -->
