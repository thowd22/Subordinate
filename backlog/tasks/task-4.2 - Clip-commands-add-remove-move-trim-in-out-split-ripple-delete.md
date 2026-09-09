---
id: TASK-4.2
title: 'Clip commands: add, remove, move, trim in/out, split, ripple delete'
status: Done
assignee:
  - '@opus-task-4.2'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 03:03'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-4.1
references:
  - docs/PLAN.md
parent_task_id: TASK-4
priority: high
ordinal: 26000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
These are the primitive edits the timeline UI, agents and plugins compose (§2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 AddClip, RemoveClip, MoveClip, TrimClipIn, TrimClipOut, SplitClip and RippleDelete exist with inverses
- [x] #2 Overlap resolution policy (overwrite trims or removes covered clips) is implemented and documented
- [x] #3 Split at a boundary or outside the clip returns a structured error, not a panic
- [x] #4 Tests cover each command plus undo for edge cases at clip boundaries
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a clip module to sub-edit with a shared track-layout helper: place items positionally, resolve overwrite overlap, rebuild items with gaps.
2. Implement AddClip, RemoveClip, MoveClip, TrimClipIn, TrimClipOut, SplitClip, RippleDelete plus the exact-restore inverse command SetTrackItems.
3. Document the overwrite overlap policy (covered clips removed, partially covered trimmed, containing clip split) in module docs.
4. Structured errors with stable edit.* codes for missing sequence/track/clip, split at a boundary or outside a clip, and unrepresentable times.
5. Tests: each command, undo/redo byte-identical round trips, boundary edge cases, registry decode.
6. Verify with cargo fmt, clippy -D warnings, cargo test -p sub-edit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-edit/src/clip.rs: AddClip, RemoveClip, MoveClip, TrimClipIn, TrimClipOut, SplitClip, RippleDelete, plus RestoreTrackItems, the exact-restore command every clip command returns as its inverse (an overwrite can trim, remove and split several clips at once, so a per-side-effect inverse would not be exact; restoring the touched tracks' item lists is, identifiers included, and its own inverse carries the post-edit lists so redo is exact too). Registered together by clip::register.

Design notes:
- Commands work on a timeline view of the track (clip plus start) and rebuild the positional OTIO-style item list afterwards. Gaps are derived from holes, so gap identity is not preserved and empty time after the last clip is not stored; both are documented on the module.
- Overwrite overlap policy (module docs, table): butt-joined untouched, fully covered removed, partially overlapped trimmed from the matching side so surviving frames keep their timeline instants, a clip strictly containing the span split with the head keeping the identity. Fades are clamped to a shortened clip; a transition inside the span is removed with the cut it blended.
- RippleDelete ripples only its own track; multi-track ripple is a history group of one command per track.
- All arithmetic is RationalTime checked_add/checked_sub, never floats and never rounding; a non-exact combination is edit.invalid_time.
- New stable codes: edit.sequence_not_found, edit.track_not_found, edit.clip_not_found, edit.media_not_found, edit.duplicate_clip, edit.invalid_split, edit.invalid_trim, edit.invalid_time.
- MoveClip takes an optional to_track and SplitClip an optional tail_id so a replayed command reproduces identical ids.
- sub-time moved from a dev-dependency to a dependency of sub-edit.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (exit 0, GStreamer env sourced for sub-media); cargo test -p sub-edit passes 51 tests (17 unit, 24 new clip_commands integration tests, 6 undo_redo, 4 doc tests).

Evidence per criterion:
1. All seven commands plus their inverse exist and undo: every mutating test in crates/sub-edit/tests/clip_commands.rs runs through round_trip, which applies via History, undoes and asserts the project JSON is byte-identical to the state before, then redoes and asserts it is byte-identical to the state after.
2. Overwrite policy implemented in Layout::overwrite and documented in the clip module docs; proved by add_overwrites_covered_clips_and_trims_the_ones_it_only_reaches, add_inside_a_clip_splits_it_and_the_head_keeps_the_identity, add_butt_joined_to_a_neighbour_touches_nothing, trim_in_earlier_overwrites_the_neighbour_it_reaches_into and move_leaves_a_hole_and_overwrites_where_it_lands.
3. split_at_a_boundary_or_outside_the_clip_is_a_structured_error asserts edit.invalid_split for at = 0, 24 (both boundaries), 30 and -6, with no panic and no change to the track.
4. Boundary edge cases covered: butt-joined placement, removing the last clip, trims that would empty a clip or leave the probed source, fade clamping on a shortened clip, ripple delete pulling a gap back, cross-track move restoring both tracks, and a refused command leaving the project untouched.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the primitive clip commands to sub-edit (crates/sub-edit/src/clip.rs): AddClip, RemoveClip, MoveClip, TrimClipIn, TrimClipOut, SplitClip and RippleDelete, all undoing through RestoreTrackItems, which restores the affected tracks' item lists exactly (identifiers included) and whose own inverse makes redo exact. Placement goes through a timeline view of the track that applies the documented overwrite policy - covered clips removed, partially overlapped clips trimmed on the matching side, a containing clip split with the head keeping its identity - and rebuilds the positional item list with gaps for the holes; all arithmetic is exact RationalTime, and failures are SubError with new stable edit.* codes (invalid_split for a split at or outside a clip boundary, invalid_trim, invalid_time, the not_found family). Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-edit (51 tests, 24 of them new).
<!-- SECTION:FINAL_SUMMARY:END -->
