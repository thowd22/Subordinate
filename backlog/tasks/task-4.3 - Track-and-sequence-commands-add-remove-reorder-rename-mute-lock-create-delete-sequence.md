---
id: TASK-4.3
title: >-
  Track and sequence commands: add, remove, reorder, rename, mute, lock,
  create/delete sequence
status: Done
assignee:
  - '@opus-task-4.3'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 03:08'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-4.1
references:
  - docs/PLAN.md
parent_task_id: TASK-4
priority: high
ordinal: 27000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Multiple sequences and tracks are MVP requirements (§2) and need first-class undoable commands.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 AddTrack, RemoveTrack, ReorderTrack, RenameTrack, SetTrackMuted, SetTrackLocked, CreateSequence, DeleteSequence, RenameSequence, SetSequenceSettings exist with inverses
- [x] #2 Removing a track with clips is refused unless force is set, and force is undoable
- [x] #3 Locked tracks reject clip commands with a structured error
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-model: add muted and locked booleans to Track (serde default so existing v1 files load), plus track_mut/track_index and sequence_mut/sequence_index lookup helpers; regenerate the golden fixture and docs/schema JSON Schema.
2. sub-edit: new commands module with track.rs and sequence.rs. Track: AddTrack, InsertTrack (exact inverse carrying the whole Track), RemoveTrack (refused when the track holds clips unless force), ReorderTrack, RenameTrack, SetTrackMuted, SetTrackLocked. Sequence: CreateSequence, InsertSequence, DeleteSequence, RenameSequence, SetSequenceSettings.
3. sub-edit: new stable error codes edit.sequence_not_found, edit.track_not_found, edit.track_locked, edit.track_not_empty, edit.invalid_index; a public lookup helper that clip commands (TASK-4.2) call so a locked track rejects them with edit.track_locked.
4. register_builtin() to add every kind to a CommandRegistry.
5. Tests: apply/undo/redo byte-identical round trips through json::to_json, envelope round trips, force removal undo, locked-track rejection, error codes.
6. Verify with cargo fmt --check, clippy pedantic -D warnings, cargo test -p sub-model -p sub-edit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Model: Track gained muted and locked booleans, both #[serde(default)] so a project file written before they existed still loads (covered by a_track_written_before_the_flags_existed_still_loads). SCHEMA_VERSION stays 1: the addition is read-compatible and v1 has not shipped, so no migration was registered. Regenerated crates/sub-model/tests/fixtures/sample-project.sub (SUB_UPDATE_GOLDEN=1) and docs/schema/project-v1.schema.json (SUB_UPDATE_SCHEMA=1) with the documented commands. Added Sequence::track_mut/track_index and Project::sequence_mut/sequence_index.

Commands (crates/sub-edit/src/commands/): track.rs holds AddTrack, InsertTrack, RemoveTrack, ReorderTrack, RenameTrack, SetTrackMuted, SetTrackLocked; sequence.rs holds CreateSequence, InsertSequence, DeleteSequence, RenameSequence, SetSequenceSettings. InsertTrack and InsertSequence are the restoring inverses: they carry the whole entity plus the index it sat at, so undo restores identifiers and contents exactly and a redone AddTrack keeps the identifier it was first given. commands::register_builtin/builtin_registry put all twelve kinds on a CommandRegistry.

AC2: RemoveTrack refuses a track holding clips with edit.track_not_empty (details carry the clip count) and leaves the project untouched; RemoveTrack::forced removes it with its clips and its inverse restores the track, its clips and its index — proved byte-for-byte against json::to_json in a_forced_removal_takes_the_clips_with_it_and_undoes_them_back and an_empty_track_is_removed_but_one_holding_clips_needs_force.

AC3: the lock is one rule, not a per-command check. commands::track_for_clip_edit is the lookup every clip command (TASK-4.2) will call, and it returns edit.track_locked with sequence_id, track_id and track_name details. track_mut, used by the header commands, deliberately succeeds on a locked track so SetTrackLocked can unlock it. Test a_locked_track_rejects_clip_commands_with_a_structured_error drives a stand-in clip command through it and asserts the refusal changed nothing.

New stable codes: edit.sequence_not_found, edit.track_not_found, edit.track_locked, edit.track_not_empty, edit.invalid_index, edit.duplicate_track, edit.duplicate_sequence.

Deliberately out of scope: removing or reordering a locked track is still allowed (the lock guards clip content only, per AC3), and deleting the last sequence is permitted since it is undoable.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (exit 0, with the scratchpad GStreamer pkg-config env); cargo test --workspace green, including 17 new tests in crates/sub-edit/tests/track_sequence_commands.rs, 4 unit tests in commands/mod.rs and 2 new sub-model unit tests.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the track and sequence command set to sub-edit: AddTrack, InsertTrack, RemoveTrack, ReorderTrack, RenameTrack, SetTrackMuted, SetTrackLocked, CreateSequence, InsertSequence, DeleteSequence, RenameSequence and SetSequenceSettings, each with an exact inverse and all twelve registered by commands::register_builtin. Track gained muted and locked in sub-model (serde-defaulted, golden fixture and JSON Schema regenerated), and commands::track_for_clip_edit gives the clip commands of TASK-4.2 a single lock check that fails with edit.track_locked; removing a track that still holds clips is refused with edit.track_not_empty unless force is set, and a forced removal undoes the clips back. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test --workspace, all clean, including 23 new tests.
<!-- SECTION:FINAL_SUMMARY:END -->
