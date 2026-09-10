---
id: TASK-30
title: Clip selection and drag-move with undoable command groups
status: Done
assignee:
  - '@opus-task-30'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 14:58'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-29
  - TASK-4.2
references:
  - docs/PLAN.md
priority: high
ordinal: 51000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Moving clips is the most frequent edit; it must feel immediate yet produce a single undo step.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Click, shift-click and marquee selection across tracks
- [x] #2 Dragging moves selected clips with live preview and commits one grouped MoveClip command on release
- [x] #3 Dragging onto a locked track or outside the sequence is refused visually
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers this: an interaction test that selects and drags a clip and asserts the resulting command group, plus a snapshot showing the selected state
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New crates/sub-ui/src/selection.rs: ClipRef (track + clip id), Selection (ordered set with select_only/toggle/extend/contains/clear), and MoveGroup { label, moves: Vec<MoveClip> } with into_commands(), plus plan_move() that turns a selection + time delta + track delta into an ordered MoveClip group and refuses locked source/target tracks or a start before zero (SubError with a stable ui.* code).
2. TimelinePanel gains selection state and a lane gesture: press on a clip selects it (shift-click toggles/extends), press on empty lane starts a marquee, dragging a selected clip starts a move. Time never leaves RationalTime: the delta is time_at_pixel(now) - time_at_pixel(origin); the track delta is an integer lane index difference.
3. Painting: selected clips get a highlight outline, the marquee draws a rubber band, a move in progress paints ghost rectangles at the previewed position, tinted with a refusal colour when the move is refused (locked track or before the head of the sequence).
4. TimelineResponse gains selection_changed and clip_move: Option<MoveGroup>, produced only on release of a legal drag; app.rs logs it like the other not-yet-wired actions.
5. Unit tests in selection.rs and timeline_panel.rs; an egui_kittest interaction test crates/sub-ui/tests/timeline_selection.rs on the shared harness covering click, shift-click, marquee, drag-move (asserting the resulting MoveClip group applied through a History group is one undo step), refusal on a locked track and before zero, plus a snapshot of the selected state.
6. Verify with cargo fmt --check, clippy -D warnings and cargo test -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in two pieces.

New crates/sub-ui/src/selection.rs holds the model half: ClipRef (track + clip id), Selection (ordered set: select_only, toggle, insert, set, clear, retain_existing), MoveRefusal (BeforeStart, LockedTrack, NoSuchTrack, UnknownClip, OutOfRange, each with a stable id and the new ui.clip_move_refused SubError code), PreviewedMove, MoveGroup, plan_move(), clips_in_marquee() and apply_move(). plan_move does the arithmetic in RationalTime and integer track indexes only - no floats - and orders the MoveClip commands so a drag right moves the right-most clip first, which is what stops a clip in the selection overwriting one that has not moved yet. apply_move wraps the whole group in History::begin_group/commit_group, so a drag of any number of clips is one undo step. A drag of zero distance is not an edit and produces no group.

crates/sub-ui/src/timeline_panel.rs gained the gesture half: a press on a clip starts a move of whatever is selected (shift toggles), a press on empty lane or on a locked track's clip starts a marquee, and the panel recomputes the plan every frame while the button is down. The plan is painted: ghost rectangles at the previewed position for a legal drag, a near-white outline on the selected clips, a rubber band for the marquee, and a red wash plus outline on the clips of a refused drag. TimelineResponse gained selection_changed, clip_move and refused; the panel still mutates nothing, and app.rs logs the group the way it logs the other not-yet-wired actions until the app owns an engine handle. handle_scrub now yields to a gesture that started in the lanes, so dragging a clip up over the ruler no longer turns into a scrub. sync() drops selected clips that have left the sequence.

Drag snapping was deliberately left out: TASK-29's snapping serves the playhead, and pulling a dragged clip edge onto a snap target is trim/drag polish rather than part of these criteria.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui green (254 unit tests plus every integration test, including the 13 in the new tests/timeline_selection.rs). The snapshot tests/snapshots/timeline_selection.png was recorded with UPDATE_SNAPSHOTS=1 on this machine's software wgpu adapter and is committed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Clip selection and drag-move on the timeline. A new sub-ui selection module models what is selected and turns a drag into an ordered group of MoveClip commands applied inside one History group, refusing a drag that would leave the sequence, cross track kinds or touch a locked track with the new ui.clip_move_refused code; the timeline panel drives it with click, shift-click, marquee and drag gestures, previewing the move as ghosts and painting a refusal in red. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-ui, including 13 new egui_kittest tests in tests/timeline_selection.rs that assert the resulting command group undoes in one step, plus the committed timeline_selection snapshot.
<!-- SECTION:FINAL_SUMMARY:END -->
