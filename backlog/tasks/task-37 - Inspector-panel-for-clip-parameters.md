---
id: TASK-37
title: Inspector panel for clip parameters
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
updated_date: '2026-09-11 04:18'
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
- [ ] #2 Edits apply live to the viewer and commit one undoable command on release
- [x] #3 Multi-selection edits apply to all selected clips
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the inspector: a committed snapshot of the panel over the sample project, and an interaction test that edits a parameter and asserts the command it issues
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-ui/src/inspector.rs: InspectorPanel painting opacity, position x/y, scale x/y, rotation, gain and fade in/out for the selected clips, all as labelled egui sliders so every field is queryable by accessibility label.
2. The panel mutates nothing. It returns an InspectorResponse { begin: Option<String>, commands: Vec<SetClipParams>, commit: bool }: a gesture opens a history group, each changed frame raises one SetClipParams per selected clip (live preview), and releasing the widget commits the group as one undo step.
3. Multi-selection: the fields read the anchor (first selected) clip; an edit builds one command per selected clip on an unlocked track, with fades clamped per clip so a shorter clip cannot be pushed into an invalid state. All numbers go through Fixed6/RationalTime, never floats in the model.
4. Add apply_edit(history, project, response) helper mirroring selection::apply_move, and wire the panel into app.rs's Inspector dock tab.
5. Tests in crates/sub-ui/tests/inspector.rs on the shared harness: a committed snapshot of the panel over the sample project, and interaction tests that drag the opacity slider and assert the SetClipParams issued, that release commits exactly one undo entry, and that a multi-selection edits every clip.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented crates/sub-ui/src/inspector.rs: InspectorPanel paints opacity, position X/Y, scale X/Y, rotation, gain and fade in/out for the selected clips as labelled egui sliders, reading the anchor (first-selected) clip. The panel mutates nothing; it returns an InspectorResponse { begin, commands, commit } and inspector::apply_edit turns that into History::begin_group / apply / commit_group, so a whole drag is applied frame by frame (live) but lands as exactly one undo entry. Values cross into the model through Fixed6::from_f64 or as exact frame counts at the sequence timebase; nothing float-valued is stored between frames. Clips on locked tracks are excluded and fades are clamped per clip so a short clip in a multi-selection cannot be pushed into an invalid state.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui green (all 21 targets, including the new tests/inspector.rs with 7 tests). New committed snapshot crates/sub-ui/tests/snapshots/inspector_clip_parameters.png shows the panel over the sample project's 'shot 1' (opacity 0.9, gain -3.0 dB, fade in 6). The interaction tests drag the opacity slider through the shared harness and assert the SetClipParams issued (sequence/track/clip ids, opacity only, no other field named), that the clip changed while the pointer was still down with no undo entry pushed yet, and that releasing left exactly one undo entry that puts every edited clip back.

AC #2 left unchecked: the live-apply and one-command-on-release halves are proven by the interaction tests through the Command API, but the app-side half cannot be proven here. SubordinateApp owns no engine handle yet, so app.rs logs the inspector's response exactly as it logs the timeline's clip moves and the media bin's actions ('not wired up yet'); applying edits on the UI thread would duplicate the engine thread that TASK-12 makes the owner of project state. Nothing therefore reaches the compositor, so no viewer refresh could be demonstrated. Wiring the panels to the engine handle is the outstanding work; it is not inspector-specific and was not added here to avoid expanding scope.

2026-09-10 supervisor: requeued. TASK-127 wired the engine into SubordinateApp, so criterion 2 (edits apply live to the viewer and commit one undoable command on release) can now be completed through the app's engine handle.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the inspector panel (crates/sub-ui/src/inspector.rs): opacity, position, scale, rotation, gain and fade fields for the selected clips, raising SetClipParams commands rather than mutating anything, with inspector::apply_edit keeping a whole drag to one undo entry and multi-selection edits applying to every selected clip on an unlocked track. Verified with cargo fmt --check, workspace clippy at -D warnings, and cargo test -p sub-ui, including a new egui_kittest snapshot of the panel over the sample project and interaction tests that drag a slider and assert the command issued, the live change under the pointer and the single undo entry on release. AC #2 is left unchecked: its app-side half (the viewer recompositing) needs the engine handle SubordinateApp does not own yet, so the inspector's response is logged there like every other panel's actions.
<!-- SECTION:FINAL_SUMMARY:END -->
