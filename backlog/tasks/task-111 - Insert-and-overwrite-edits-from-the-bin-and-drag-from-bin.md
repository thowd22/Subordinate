---
id: TASK-111
title: Insert and overwrite edits from the bin and drag-from-bin
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:07'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-30
  - TASK-35
references:
  - docs/PLAN.md
priority: high
ordinal: 32500
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Getting media onto the timeline with predictable three-point behaviour.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Dragging a media item from the bin onto a track creates a clip at the drop time
- [ ] #2 Comma inserts (ripples later clips) and period overwrites at the playhead on the target track
- [ ] #3 Audio tracks receive audio-only items; video tracks refuse them with a hint
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers insert and overwrite from the bin: interaction tests asserting the resulting clips, plus a snapshot of the drag feedback if one is drawn
<!-- AC:END -->
