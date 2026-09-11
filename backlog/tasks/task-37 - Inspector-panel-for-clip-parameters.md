---
id: TASK-37
title: Inspector panel for clip parameters
status: Done
assignee:
  - '@opus-task-37'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-11 04:46'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-127
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
- [x] #1 Selecting a clip shows opacity, position, scale, rotation, gain and fade fields
- [x] #2 Edits apply live to the viewer and commit one undoable command on release
- [x] #3 Multi-selection edits apply to all selected clips
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the inspector: a committed snapshot of the panel over the sample project, and an interaction test that edits a parameter and asserts the command it issues
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. TASK-127 already wired SubordinateApp to the engine and app.rs::apply_inspector turns the panel's InspectorResponse into begin_group/apply/commit_group on the session, so the remaining work for AC #2 is proof at the app level plus the stale doc comment that still says the inspector has no widgets.
2. Expose the timeline panel (pub fn timeline) on SubordinateApp the way viewer() is exposed, so an app-level test can select a clip before driving the inspector.
3. Add an app-level test in crates/sub-ui/tests/app_engine.rs: open the real window on a copy of the sample project, select the first video clip, drag the inspector's Opacity slider, and assert mid-drag that the sequence the compositor renders and the engine's project both carry the new opacity with nothing yet on the undo stack (live), then on release that the engine has exactly one undo entry labelled 'Change opacity' and that undoing it from the Edit menu restores the clip.
4. Refresh the dock_ui doc comment, run fmt/clippy/tests, then finalize.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented crates/sub-ui/src/inspector.rs: InspectorPanel paints opacity, position X/Y, scale X/Y, rotation, gain and fade in/out for the selected clips as labelled egui sliders, reading the anchor (first-selected) clip. The panel mutates nothing; it returns an InspectorResponse { begin, commands, commit } and inspector::apply_edit turns that into History::begin_group / apply / commit_group, so a whole drag is applied frame by frame (live) but lands as exactly one undo entry. Values cross into the model through Fixed6::from_f64 or as exact frame counts at the sequence timebase; nothing float-valued is stored between frames. Clips on locked tracks are excluded and fades are clamped per clip so a short clip in a multi-selection cannot be pushed into an invalid state.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui green (all 21 targets, including the new tests/inspector.rs with 7 tests). New committed snapshot crates/sub-ui/tests/snapshots/inspector_clip_parameters.png shows the panel over the sample project's 'shot 1' (opacity 0.9, gain -3.0 dB, fade in 6). The interaction tests drag the opacity slider through the shared harness and assert the SetClipParams issued (sequence/track/clip ids, opacity only, no other field named), that the clip changed while the pointer was still down with no undo entry pushed yet, and that releasing left exactly one undo entry that puts every edited clip back.

AC #2 left unchecked: the live-apply and one-command-on-release halves are proven by the interaction tests through the Command API, but the app-side half cannot be proven here. SubordinateApp owns no engine handle yet, so app.rs logs the inspector's response exactly as it logs the timeline's clip moves and the media bin's actions ('not wired up yet'); applying edits on the UI thread would duplicate the engine thread that TASK-12 makes the owner of project state. Nothing therefore reaches the compositor, so no viewer refresh could be demonstrated. Wiring the panels to the engine handle is the outstanding work; it is not inspector-specific and was not added here to avoid expanding scope.

2026-09-10 supervisor: requeued. TASK-127 wired the engine into SubordinateApp, so criterion 2 (edits apply live to the viewer and commit one undoable command on release) can now be completed through the app's engine handle.

2026-09-10 (requeue): AC #2 closed at the app level. TASK-127 had already given SubordinateApp an engine session and app.rs::apply_inspector turns the panel's InspectorResponse into begin_group / apply / commit_group on it, so the work here was the missing proof plus two small gaps: SubordinateApp now exposes the timeline panel (pub fn timeline, mirroring viewer()) so another surface can say which clips the inspector is pointing at, and the stale dock_ui doc comment claiming the inspector has no widgets is gone.

New test crates/sub-ui/tests/app_engine.rs::an_inspector_drag_edits_the_viewer_live_and_lands_as_one_undo_step drives the assembled window: it opens the sample project through the engine, selects the first clip of an unlocked video track, and drags the docked inspector's Opacity slider. Mid-drag, with the pointer still down, it asserts the opacity in the sequence the compositor renders from (SubordinateApp::sequence, what composite() draws each frame after sync_project raises needs_composite) has changed, that project.get on the window's own Command API reports the same live value, and that the engine's history has undo_len 0 with the gesture's group still open. On release it asserts undo_len 1 labelled 'Change opacity', and then undoes from the Edit menu and asserts the clip is back to 900000 micro-units both in the window and through the Command API.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (exit 0); cargo test -p sub-ui green, 30 targets, including app_engine (4 tests) and the existing inspector suite.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The inspector panel (crates/sub-ui/src/inspector.rs) shows opacity, position, scale, rotation, gain and fade for the selected clips and raises SetClipParams rather than mutating anything; inspector::apply_edit and app.rs::apply_inspector keep a whole drag live frame by frame while landing it as exactly one undo entry, and a multi-selection edits every selected clip on an unlocked track. With the engine now wired into SubordinateApp, the app-level half is proven too: a new test in crates/sub-ui/tests/app_engine.rs drags the docked inspector's Opacity slider in the assembled window and asserts the live value mid-drag in both the sequence the compositor renders and the Command API's project, no undo entry until release, exactly one 'Change opacity' entry after it, and a clean restore via the Edit menu's Undo. SubordinateApp also exposes its timeline panel so the selection can be set from outside, and the stale dock doc comment is corrected. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, and cargo test -p sub-ui (all green), including the committed snapshot crates/sub-ui/tests/snapshots/inspector_clip_parameters.png.
<!-- SECTION:FINAL_SUMMARY:END -->
