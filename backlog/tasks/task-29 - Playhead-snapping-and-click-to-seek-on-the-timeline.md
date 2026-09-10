---
id: TASK-29
title: 'Playhead, snapping and click-to-seek on the timeline'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 08:07'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-28
  - TASK-23
references:
  - docs/PLAN.md
priority: high
ordinal: 50000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Precise placement relies on snapping to clip edges, markers and the playhead.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Clicking the ruler seeks; dragging scrubs with viewer updates
- [ ] #2 Snap targets: clip edges, markers, playhead, sequence start; toggle with S
- [ ] #3 Snap threshold is in pixels and independent of zoom
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers this: an interaction test that clicks the ruler and asserts the playhead time, plus a snapshot of the panel with the playhead drawn
<!-- AC:END -->
