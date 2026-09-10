---
id: TASK-43
title: Dockable panel layout with persistence
status: Done
assignee:
  - '@opus-task-43'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 08:47'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-22
  - TASK-28
references:
  - docs/PLAN.md
priority: medium
ordinal: 64000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Panels: bin, timeline, viewer, inspector, export (§5.7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 egui_dock (or equivalent) hosts all panels with drag-to-rearrange and tab groups
- [x] #2 Layout persists per user and resets via a menu item
- [x] #3 Default layout matches the plan's panel list
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add egui_dock 0.21 (serde feature), serde and serde_json to sub-ui.
2. New module sub-ui/src/dock.rs: Panel enum (MediaBin, Timeline, Viewer, Inspector, Export) matching PLAN §5.7, a DockLayout wrapping egui_dock::DockState<Panel> with drag-to-rearrange and tab groups, and a default layout (bin left, viewer centre, inspector+export tabbed right, timeline across the bottom).
3. Persistence: layout.json in the per-user config directory (same SUBORDINATE_CONFIG_DIR/keymap.rs conventions). Load reports problems as SubError with new stable codes ui.layout_parse / ui.layout_unreadable / ui.layout_unwritable and falls back to the default layout; save only writes when the layout actually changed.
4. Repair loaded layouts that are missing panels (push them back) instead of throwing the whole layout away.
5. Wire into app.rs: the dock hosts every panel, a View menu offers 'Reset layout', and the layout is persisted from eframe's save/on_exit hooks. Viewer draws for real; panels whose state the app does not own yet show a placeholder body.
6. Tests: default layout contains exactly the plan's panels, JSON round-trip, reset, repair of a missing panel, unreadable/malformed file handling, save-only-when-changed, plus a kittest paint of the dock.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-ui/src/dock.rs (new) and wired into crates/sub-ui/src/app.rs.

Design decisions:
- egui_dock 0.21 (the release built against egui 0.36) with its serde feature. Panel is a small enum whose serialised names (media_bin, timeline, viewer, inspector, export) are the stable part of the file format; DockLayout wraps DockState<Panel>.
- Default layout: media bin left (0.22), viewer centre, inspector and export grouped as tabs on the right, timeline across the bottom (0.62). Tabs are draggable and groupable; close and add buttons are off, because the plan's panel list is fixed and a closed panel would have no way back.
- Persistence is layout.json in the same per-user config directory keymap.toml uses (SUBORDINATE_CONFIG_DIR, else XDG/APPDATA/Application Support). Written from eframe's save hook (periodic and on exit) and only when the serialised layout differs from what was last read or written.
- egui_dock leaves NaN in the rectangle of a node it has not painted yet, and JSON has no NaN: serde_json writes null and then refuses to read it back as f32, which would have made every layout file unreadable. to_json grounds the rect/viewport nulls to zero (they are recomputed from the split fractions on the first painted frame) and leaves every other null alone, so a real Option still round-trips.
- Failure handling follows keymap.rs: LoadedLayout carries SubErrors with new stable codes ui.layout_parse, ui.layout_unreadable, ui.layout_unwritable, and a bad file costs the arrangement, never the session. A file that parses but lost or duplicated a panel is repaired rather than discarded.
- The app now owns an empty Project plus the media bin and timeline panels so the dock hosts the real widgets; the inspector and export tabs draw a placeholder line until their own tasks land. Bin and timeline actions are logged rather than applied, because the app does not own an engine handle yet.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added a dockable panel layout to sub-ui: egui_dock 0.21 hosts the media bin, timeline, viewer, inspector and export panels of docs/PLAN.md 5.7 with draggable, groupable tabs, and the arrangement is remembered per user in layout.json beside keymap.toml and reset from the View menu.

Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings (both clean) and cargo test -p sub-ui (206 unit tests plus every integration suite passing). New coverage: 17 unit tests in dock.rs (default layout is exactly the plan's panel list, inspector and export share a tab group, JSON round-trip, no NaN rectangles are written, dirty tracking, reset, repair of a missing or duplicated panel, missing/malformed/unreadable files) and 4 headless integration tests in tests/dock_layout.rs that paint the dock (every visible panel drawn, one panel per tab group), move a tab as a drag would and see it drawn in its new group, save and reload a rearranged layout, and click 'Reset layout' through AccessKit to see the default arrangement come back.
<!-- SECTION:FINAL_SUMMARY:END -->
