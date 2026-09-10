---
id: TASK-67
title: Pop-out viewer as an egui deferred viewport sharing the compositor texture
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:07'
labels:
  - ui
milestone: m-5
dependencies:
  - TASK-22
  - TASK-43
  - TASK-7
references:
  - docs/PLAN.md
priority: high
ordinal: 88000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Preview on a second display is an MVP requirement (§2). The spike proved the approach; this makes it a feature.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Viewer menu item pops the viewer into its own OS window showing the same texture with no extra render pass
- [ ] #2 Closing the pop-out returns the viewer to the dock
- [ ] #3 Pop-out receives playback shortcuts
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the pop-out: a snapshot of the viewer content painted through the harness, and an interaction test for the pop-out toggle. The deferred viewport itself is verified by TASK-118 on real hardware
<!-- AC:END -->
