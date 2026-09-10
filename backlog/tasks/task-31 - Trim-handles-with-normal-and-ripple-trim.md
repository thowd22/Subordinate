---
id: TASK-31
title: Trim handles with normal and ripple trim
status: Done
assignee:
  - '@opus-task-31'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 16:32'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-30
references:
  - docs/PLAN.md
priority: high
ordinal: 52000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Trimming in and out points is core editing; ripple trim keeps downstream clips contiguous.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Edge hover shows trim cursor; drag trims in or out point clamped to source range
- [x] #2 Holding a modifier performs ripple trim shifting later clips on the track
- [x] #3 Trim commits TrimClipIn/Out (grouped with moves for ripple) and is undoable
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers this: an interaction test that drags a trim handle and asserts the trimmed times, plus a snapshot showing the handles
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-ui/src/trim.rs: TrimEdge, TrimRefusal (ui.clip_trim_refused code), TrimStep (TrimClipIn/TrimClipOut/MoveClip), TrimGroup and plan_trim/apply_trim. Deltas are exact RationalTime, clamped to the clip's source range (media duration) and to a one-frame minimum; ripple shifts the trimmed clip and every later clip on the track so the track stays contiguous; commands are ordered so no clip overwrites one that has not moved yet.
2. Timeline panel: trim handles at clip edges (TRIM_HANDLE_PX), hover detection setting CursorIcon::ResizeHorizontal and exposed as hovered_trim(), a Gesture::Trim started by a press on a handle, alt as the ripple modifier, per-frame plan with a ghost preview, and TimelineResponse::clip_trim / trim_refused.
3. Paint handles on selected clips' trimmed-capable edges and highlight the hovered one; paint the previewed trimmed span while dragging.
4. Wire app.rs to log the trim group like clip_move; re-export from lib.rs.
5. Tests: unit tests in trim.rs for clamping, ripple ordering and single-undo-step application; an egui_kittest interaction test in crates/sub-ui/tests/timeline_trim.rs that drags a handle and asserts trimmed times (normal and ripple) plus a snapshot showing the handles.
6. Verify with cargo fmt --check, clippy -D warnings, cargo test -p sub-ui -p sub-edit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
New crates/sub-ui/src/trim.rs holds the model half: TrimEdge (In/Out), TrimRefusal (LockedTrack, UnknownClip, OutOfRange, each with a stable id and the new ui.clip_trim_refused SubError code), TrimStep (TrimClipIn / TrimClipOut / MoveClip), TrimGroup, plan_trim() and apply_trim(). All the arithmetic is RationalTime at the sequence timebase - no floats. Running out of source is CLAMPED, not refused: the offset is pulled inside bounds set by the clip's source range (the probed media duration, rounded so a cross-rate source never over-reaches), a one-frame minimum length, and, for a normal in-point trim, sequence time zero. An unprobed source has no known end, so the out point cannot grow past the frames the clip already uses. A drag clamped to nothing produces no group and no undo step.

Ripple (alt held) keeps the track contiguous. Out point: the tail moves by d and every later clip on the track moves by d. In point: the clip's head stays on the instant it was on - the clip shortens (or lengthens) and it and every later clip slide by -d. The trimmed clip's own MoveClip and the trim are ordered against each other so nothing ever overwrites a clip that has not moved yet: a leftward ripple trims first and then slides the clips left in start order, a rightward one slides them right (right-most first) and trims last. apply_trim wraps the whole list in History::begin_group/commit_group, so a ripple trim of any size is one undo step.

timeline_panel.rs grew Gesture::Trim beside Marquee and Move. trim_target_at() picks the nearer edge within TRIM_HANDLE_PX of a clip's rectangle (a clip narrower than two handles splits its width between them), begin_lane_gesture prefers it over a move so an edge press trims rather than drags, update_trim_hover() records hovered_trim() and asks for CursorIcon::ResizeHorizontal, and the gesture re-plans every frame with the alt modifier read from egui, painting the trimmed span as a ghost. Handles are painted only on the clip under the pointer, so the timeline at rest is not covered in grips and the committed timeline_selection snapshot is unchanged. TimelineResponse gained clip_trim and trim_refused; like clip_move the panel plans and never applies, and app.rs logs the group until the engine handle exists.

Modifier choice: alt is the ripple modifier because shift already means 'add to the selection' on a lane press.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui -p sub-edit all green, including 13 new unit tests in trim.rs and 9 new egui_kittest tests in crates/sub-ui/tests/timeline_trim.rs (hover reports the edge, normal in/out trims, source clamping, alt ripple in and out, a zero drag is not an edit, a locked track offers no handle) plus the newly recorded crates/sub-ui/tests/snapshots/timeline_trim_handles.png. Snapshots render here on lavapipe.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Trim handles on the timeline, with normal and ripple trim. A new sub-ui trim module turns an edge drag into an ordered TrimClipIn/TrimClipOut plus, when alt is held, the MoveClip commands that keep the rest of the track contiguous, all applied inside one History group so a ripple trim is one undo step; the offset is exact RationalTime clamped to the clip's source range and a one-frame minimum rather than refused, with ui.clip_trim_refused reserved for a locked track or a clip that has left the sequence. The timeline panel grew a Trim gesture: hovering a clip edge shows the resize cursor and paints the grips, the drag previews the trimmed span as a ghost, and the plan reaches the caller as TimelineResponse::clip_trim. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-ui -p sub-edit, including 13 trim unit tests and 9 egui_kittest tests in tests/timeline_trim.rs plus the committed timeline_trim_handles snapshot.
<!-- SECTION:FINAL_SUMMARY:END -->
