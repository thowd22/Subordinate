---
id: TASK-4.4
title: 'Parameter, marker, media and bin commands'
status: Done
assignee:
  - '@opus-task-4.4'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 04:29'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-4.2
  - TASK-4.3
references:
  - docs/PLAN.md
parent_task_id: TASK-4
priority: medium
ordinal: 28000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Inspector edits, markers and bin organisation must be undoable and scriptable like everything else.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 SetClipParams (opacity, transform, gain, fades), AddMarker, MoveMarker, RemoveMarker, ImportMedia, RemoveMedia, RelinkMedia, CreateBin, MoveToBin, RenameBin exist with inverses
- [x] #2 SetClipParams validates via Clip::validate before applying
- [x] #3 Tests cover undo for each
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add commands/params.rs: SetClipParams (opacity, transform, gain, fade_in, fade_out as optional fields), validating the candidate clip via Clip::validate before assigning; inverse carries the previous values of the same fields.
2. Add commands/marker.rs: MarkerTarget (sequence or clip), AddMarker/MoveMarker/RemoveMarker with exact inverses (removal's inverse is AddMarker carrying the whole marker and its index).
3. Add commands/media.rs: ImportMedia, InsertMedia (restore command), RemoveMedia (refuses media still used by clips unless forced), RelinkMedia (path, hash, offline) with exact inverses.
4. Add commands/bin.rs: CreateBin, InsertBin, RemoveBin (empty unless forced), MoveToBin, RenameBin; root bin is never removed or moved.
5. New stable error codes in sub_edit::codes for bin/media/marker failures; register every new kind in register_builtin and re-export from lib.rs.
6. Tests: unit tests per module plus an integration test proving byte-identical undo/redo through the project file for every new command.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in four new modules under crates/sub-edit/src/commands: params.rs (SetClipParams), marker.rs (MarkerTarget, AddMarker, MoveMarker, RemoveMarker), media.rs (ImportMedia, InsertMedia, RemoveMedia, RelinkMedia, Filing) and bin.rs (CreateBin, InsertBin, RemoveBin, RenameBin, MoveToBin). All fourteen kinds are registered in register_builtin and decode from their envelopes.

Design notes:
- SetClipParams names each parameter optionally, builds the candidate clip, runs Clip::validate on it and only then writes it; the inverse names exactly the same fields with their previous values, so undoing one inspector edit does not revert another.
- Markers live on sequences and on clips, so one tagged MarkerTarget covers both; a clip target goes through track_for_clip_edit, which makes markers on a locked track as protected as the clips they annotate.
- Restoring commands carry whole entities, matching the InsertTrack/InsertSequence convention: InsertMedia carries the item, its index in Project::media and its bin filing; InsertBin carries the whole subtree.
- Destructive edits need force: RemoveMedia refuses media clips still use (edit.media_in_use), RemoveBin refuses a non-empty bin (edit.bin_not_empty), and the root bin can never be removed (edit.root_bin).
- MoveToBin refuses an item filed in no bin, because there would be no bin for undo to return it to; ImportMedia always files what it imports.
- New stable codes: edit.marker_not_found, edit.duplicate_marker, edit.duplicate_media, edit.media_in_use, edit.bin_not_found, edit.duplicate_bin, edit.bin_not_empty, edit.root_bin.

Validation: cargo test -p sub-edit (101 tests incl. 22 new integration tests and 11 doctests) passes; cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings exits 0.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the parameter, marker, media and bin command set to sub-edit: SetClipParams (opacity, transform, gain, both fades), AddMarker/MoveMarker/RemoveMarker over a MarkerTarget covering sequence and clip markers, ImportMedia/InsertMedia/RemoveMedia/RelinkMedia, and CreateBin/InsertBin/RemoveBin/RenameBin/MoveToBin, each with an exact inverse and all fourteen registered in register_builtin. SetClipParams validates the candidate clip through Clip::validate before writing it, and destructive removals need force. Verified with 22 new integration tests that apply, undo and redo every command and assert the project file is byte-identical at each hop, plus error-path tests for the eight new stable codes; cargo test -p sub-edit, cargo fmt --all --check and cargo clippy --workspace --all-targets -- -D warnings all pass.
<!-- SECTION:FINAL_SUMMARY:END -->
