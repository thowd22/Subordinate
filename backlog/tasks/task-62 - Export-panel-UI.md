---
id: TASK-62
title: Export panel UI
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:07'
labels:
  - ui
  - export
milestone: m-4
dependencies:
  - TASK-60
  - TASK-61
  - TASK-43
references:
  - docs/PLAN.md
priority: medium
ordinal: 83000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users pick a preset, range and output path and watch progress.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Panel offers preset, sequence, in/out range, output path and encoder override
- [ ] #2 Progress bar, ETA and cancel button wired to the export job
- [ ] #3 Recent export list with open-folder action
- [ ] #4 Exporter plugin presets from the exporter WIT world appear in the preset list (moved from TASK-78)
- [ ] #5 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the export panel: a committed snapshot of the panel, and an interaction test that changes a setting and asserts the export request it builds
<!-- AC:END -->
