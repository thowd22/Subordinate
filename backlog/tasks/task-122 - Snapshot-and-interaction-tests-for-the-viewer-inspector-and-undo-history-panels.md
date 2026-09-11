---
id: TASK-122
title: >-
  Snapshot and interaction tests for the viewer, inspector and undo history
  panels
status: Done
assignee:
  - '@opus-task-122'
created_date: '2026-09-09 18:21'
updated_date: '2026-09-11 06:48'
labels:
  - ui
  - test
milestone: m-2
dependencies:
  - TASK-119
  - TASK-22
  - TASK-37
  - TASK-42
priority: medium
ordinal: 142000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Viewer shows a rendered frame from the software compositor; the inspector edits clip parameters; the history panel lists commands. Cover all three once the inspector lands.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Snapshots: viewer showing frame 10 of the sample project with timecode overlay, inspector for a selected clip, history panel after three edits
- [x] #2 Interaction tests: scrub bar drag changes the playhead, editing opacity in the inspector commits one command, clicking a history entry jumps state
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-ui/tests/viewer_panel.rs: composite frame 10 of the committed sample sequence through sub-render's Compositor (synthetic source pictures, since the fixture's media paths are not real files), read the canvas back, upload it as an egui texture and snapshot the ViewerPanel with the playhead on frame 10 so the timecode overlay reads 00:00:00:10; plus a kittest interaction test that drags the scrub bar and asserts the playhead moved to the frame under the pointer.
2. Add a history-panel snapshot after three real commands applied through sub-edit::History, and a kittest interaction test that clicks a history row by label and performs the jump against the real project.
3. Confirm the inspector half (snapshot of a selected clip, opacity drag committing exactly one command) is already covered in tests/inspector.rs and extend only if a gap shows.
4. Keep every snapshot inside the committed size budget enforced by tests/ui_harness.rs.
5. Verify with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
New file crates/sub-ui/tests/viewer_panel.rs. Snapshot viewer_frame_ten.png: the sample project's first sequence composited at frame 10 by sub-render's software compositor, read back, uploaded as an egui texture and painted by ViewerPanel with the playhead on that frame, so the PNG shows the picture letterboxed on black with the readout on 00:00:00:10. The harness renders the UI on its own wgpu device, so the canvas crosses devices through a readback and an upload; the editor shares one device and samples the compositor texture directly. The fixture's media paths are documentation rather than files, so the layers are given a generated ramp picture instead of decoded frames (the decode path is covered by sample_project_render.rs), and the canvas is composited at 640x360 - the fixture's own 16:9 - to keep the readback and the committed PNG small. Interaction tests in the same file: a real pointer press-and-move on the scrub bar puts the playhead at the frame under the pointer, follows it while the button is down and leaves it there on release; the timecode readout tracks the playhead. The playhead is view state, not a project mutation, so it is asserted on the panel and is deliberately not a Command.

crates/sub-ui/tests/history_panel.rs gains the two harness halves: snapshot history_panel_three_edits.png after three real commands applied through sub-edit::History (add track, rename, mute), and a kittest test that clicks a row by the label a user reads and performs the jump against the real project - back two steps to the first command, then forward again - asserting on the project's track rather than on the returned action alone.

The inspector half of both criteria was already delivered by TASK-37 and is re-verified here rather than duplicated: tests/inspector.rs holds the inspector_clip_parameters snapshot over the sample project and dragging_the_opacity_slider_edits_live_and_commits_one_undo_step, which asserts the whole drag is exactly one entry in the undo stack.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui all green (including the two new snapshots, recorded with UPDATE_SNAPSHOTS=1 and inspected). The new PNGs are 24786 and 17507 bytes, inside the per-snapshot and directory budgets that tests/ui_harness.rs enforces. This machine has a software wgpu adapter, so the snapshot halves really ran here; on a machine with no adapter they report and skip, as the shared harness requires.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Covered the viewer, inspector and undo history panels with egui_kittest snapshot and interaction tests through the shared harness. Added crates/sub-ui/tests/viewer_panel.rs (viewer_frame_ten.png: the sample sequence composited at frame 10 by sub-render and painted with the timecode readout on 00:00:00:10; plus scrub-bar drag and timecode interaction tests) and two harness halves in tests/history_panel.rs (history_panel_three_edits.png, and clicking a history row by label to jump the real project back two commands and forward again). The inspector half was already covered by tests/inspector.rs and was re-verified rather than duplicated. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-ui, all green on this machine's software wgpu adapter.
<!-- SECTION:FINAL_SUMMARY:END -->
