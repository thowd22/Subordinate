---
id: TASK-67
title: Pop-out viewer as an egui deferred viewport sharing the compositor texture
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
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
<!-- AC:END -->
