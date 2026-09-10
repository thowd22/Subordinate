---
id: TASK-120
title: Snapshot and interaction tests for the timeline panel
status: Done
assignee:
  - '@opus-task-120'
created_date: '2026-09-09 18:21'
updated_date: '2026-09-10 18:43'
labels:
  - ui
  - test
milestone: m-2
dependencies:
  - TASK-119
  - TASK-30
  - TASK-32
priority: high
ordinal: 140000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The timeline is the most complex custom-painted surface and the most likely to regress visually. Cover it with kittest snapshots at three zoom levels plus scripted interactions that assert model state through the engine, not pixels.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Snapshots: empty sequence, the sample project at fit-to-window zoom, and single-frame zoom around the playhead
- [x] #2 Interaction tests: click selects a clip, drag moves it and produces one undo step, Ctrl+K splits at the playhead, S toggles snapping; each asserts project state via the Command API
- [x] #3 Tests run in under 20 seconds total on the hosted Linux runner
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-ui/tests/timeline_regression.rs on the shared tests/support kittest harness.
2. Snapshots (AC1): timeline_empty_sequence, timeline_fit_to_window and timeline_frame_zoom, all painted over the committed sample project (empty case: a sequence with tracks and no clips), at fit-to-window and single-frame zoom around the playhead.
3. Interactions (AC2): drive real pointer and key events through the panel plus the default ShortcutMap (the dispatch SubordinateApp::apply_shortcuts makes) - click selects, drag moves and is one History entry, Ctrl+K splits at the playhead, S toggles snapping - and assert on the project after applying each planned group through sub_edit::History with apply_move/apply_split.
4. Record the reference PNGs if a wgpu adapter exists here; otherwise leave AC1 unproven and note that CI records them.
5. Verify with cargo fmt --check, clippy -D warnings and cargo test -p sub-ui, timing the suite for AC3.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-ui/tests/timeline_regression.rs (7 tests) on the shared kittest harness, plus three committed references in crates/sub-ui/tests/snapshots/.

AC1: timeline_empty_sequence.png (a fresh sequence at the fixture timebase: ruler, playhead, empty lane area), timeline_fit_to_window.png (the sample project's Main sequence fitted to the lane width on the second frame, all three tracks and the act-two marker visible) and timeline_frame_zoom.png (ZoomLevel::MAX around frame 96, the cut between shot 1 and shot 2, scrolled back half a viewport so the playhead is centred; the test asserts the zoom is frame resolvable). 19-26 KB each, inside the per-file and directory budgets ui_harness enforces.

AC2: the four gestures are driven as input, not as method calls. The pointer does the click and the drag; Ctrl+K and S are real key events that go through ShortcutMap::poll and the same dispatch SubordinateApp::apply_shortcuts makes, so a rebinding or a dropped arm fails here too. Every planned group is applied through the Command API (selection::apply_move, split::apply_split into a real sub_edit::History) and the assertions read the project back: the drag moves 'lower third' 24 frames and is one undo entry that one undo reverses; Ctrl+K cuts the selected 'shot 1' into 0..40 and 40..96, leaves the unselected tracks alone, and is one undo entry; S turns snapping off (a click 3 frames past the cut then lands exactly there) and on again (the same click is pulled onto the cut).

AC3: 0.27 s for all seven tests on lavapipe (VK_ICD_FILENAMES=lvp_icd.json), the same software adapter CI renders with, and 0.25 s on this machine's own adapter - two orders of magnitude under the 20 s budget. The references were recorded on the local adapter and then re-verified unchanged under lavapipe, so they are already known to survive a backend change.

Checks: cargo fmt --all --check clean; cargo clippy -p sub-ui --all-targets -- -D warnings clean (sub-ui is the only crate touched, so the workspace-wide run was not repeated on this loaded machine); cargo test -p sub-ui all green (25 binaries, no failures).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Gave the timeline panel its own regression suite: crates/sub-ui/tests/timeline_regression.rs, seven tests over the committed sample project through the shared egui_kittest harness. Three committed snapshots pin the pictures a user sits at (empty sequence, whole cut fitted to the window, single-frame zoom around the playhead) and four interaction tests drive the pointer and real Ctrl+K / S key events through the shipped ShortcutMap, applying every planned group through the Command API and asserting on the project: click selects, a drag moves a clip in one undoable step, Ctrl+K cuts the selected clip at the playhead in one undoable step, and S toggles snapping so the next ruler click lands on or off the cut. Verified with cargo test -p sub-ui (all green), the suite timed at 0.27 s on lavapipe and 0.25 s on the local adapter, cargo fmt --all --check and cargo clippy -p sub-ui --all-targets -D warnings.
<!-- SECTION:FINAL_SUMMARY:END -->
