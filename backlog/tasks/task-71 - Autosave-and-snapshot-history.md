---
id: TASK-71
title: Autosave and snapshot history
status: In Progress
assignee:
  - '@opus-task-71'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 13:46'
labels:
  - core
  - ui
milestone: m-5
dependencies:
  - TASK-3.6
  - TASK-12
references:
  - docs/PLAN.md
priority: medium
ordinal: 92000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Crash recovery and cheap versioning.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Autosave writes to the sidecar dir every N seconds after changes without blocking the UI
- [ ] #2 On open, a newer autosave than the project file prompts to recover
- [ ] #3 Snapshots keep the last K autosaves and can be restored from a menu
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New sub-edit module `autosave`: SnapshotStore over a project's sidecar dir (name.sub -> name.sub.d/autosave/), writing deterministic sub-model JSON to a .tmp and renaming into place so a reader never sees a half file, with snapshot file names carrying the save time and engine revision.
2. Snapshot metadata (path, revision, saved_at, label) plus list() newest-first, load() back into a Project, and pruning to the last K snapshots (AC 3 data model for the restore menu).
3. Autosave worker: a thread that subscribes to the engine's ChangeEvent bus, marks the project dirty, and every N seconds writes one snapshot from an EngineHandle::snapshot() Arc. No UI thread work at all, the engine is never blocked (writes come off an Arc snapshot, not the queue), and stopping flushes a final snapshot (AC 1).
4. Recovery check on open: compare the newest snapshot against the project file's mtime/revision and return a Recovery describing the choice a UI prompt renders, with recover()/discard() (AC 2 decision logic; sub-ui is still a shell with no project-open flow, so no egui prompt is wired).
5. Stable error codes edit.autosave_failed and edit.snapshot_not_found in sub_edit::codes; no floats, no new project mutation (restore hands back a Project the caller opens).
6. Tests: unit tests in the module plus tests/autosave.rs covering interval writes after changes, pruning to K, list/load round-trip, recovery detection both ways, and that autosave never blocks the engine. Document the autosave/ sidecar layout in docs/DEVELOPMENT.md.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation (crates/sub-edit/src/autosave.rs, exported from sub_edit):

- SnapshotStore owns <project>.sub.d/autosave/. Each snapshot is a complete deterministic .sub file named autosave-<unix millis>-r<revision>.sub (fixed-width millis so a name sort is a time sort), written to a .tmp and renamed into place, then pruned to the newest K. list() is newest-first, latest() is the head, load() reads a snapshot back through the ordinary sub_model::json loader and migrations, clear() drops the history.
- Autosave is the worker thread ('sub-autosave'): it subscribes to the engine's ChangeEvent bus, and at most once per interval, and only when the engine's published revision moved, writes an EngineHandle::snapshot() Arc. Nothing runs on the UI thread and nothing goes through the engine queue, so neither is blocked. Stopping (or dropping) flushes the change the last incomplete interval did not reach. The engine's published revision, not the last event, decides what to write, because an event can still be in flight when a stop asks for the final snapshot; the revision is read before the project so the bytes written are never older than the revision recorded for them.
- A failed write (read-only or full sidecar) is recorded in AutosaveStatus::last_error and retried next interval, never raised: it must not stop an edit. New stable codes edit.autosave_failed and edit.snapshot_not_found.
- check_for_recovery(path) / check_for_recovery_within(path, tolerance) is the open-time question: a snapshot newer than the project file (or snapshots with no project file at all) returns a Recovery carrying label(), snapshot(), project_modified(), ahead_by(), store(), plus the two answers recover() (load the snapshot) and discard() (drop the history, leave the file). Recovery needs a tolerance (RECOVERY_TOLERANCE, 2 s): measured on this machine, a file's mtime can read ~1.5 ms EARLIER than SystemTime::now() at the moment of the write, and some file systems keep whole seconds, so a strict comparison prompts after a save made moments after an autosave. Tests use check_for_recovery_within to see millisecond differences.
- Restoring is deliberately not a Command: a snapshot is a whole project, not a mutation of the open one, so Snapshot::load() hands back a Project the caller opens with a fresh Engine and history, as opening a file does. No floats and no new project mutation path.
- docs/DEVELOPMENT.md 'Project files' gained an 'Autosave and snapshot history' section describing the layout, the worker and the recovery/restore flow.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-edit -p sub-command -p sub-model all green (sub-edit: 51 unit + 8 new integration tests in crates/sub-edit/tests/autosave.rs + 15 doctests), and the autosave integration test ran five times in a row with no flake.

Acceptance criteria evidence:

AC 1 (checked): tests/autosave.rs::a_change_is_autosaved_to_the_sidecar_directory_within_an_interval proves one rename produces exactly one snapshot inside sidecar_dir(project)/autosave that loads back with the renamed sequence; an_unchanged_project_is_never_autosaved proves five idle intervals write nothing ('after changes'); many_changes_inside_one_interval_collapse_into_one_snapshot proves 20 commands inside one interval become a single snapshot at the newest revision; autosaving_never_blocks_the_thread_that_edits runs 200 commands from a separate editing thread while the worker writes continuously (interval 1 ms) and asserts the slowest command still returns in well under half a second, with snapshots written meanwhile and no write error.

AC 2 (NOT checked): the decision and the prompt's content are implemented and proven headlessly — an_autosave_newer_than_the_project_file_offers_recovery (a session that edits and stops without saving is offered back on the next open, recover() returns the autosaved project, the project file is untouched, discard() silences the next open), a_project_saved_after_its_last_autosave_opens_without_a_prompt, snapshots_without_a_project_file_are_offered_too. What is missing is the prompt itself: crates/sub-ui is still the eframe shell from TASK-10 with no project open/save flow and no dialog to hang this on, so nothing renders Recovery::label() to a user yet. The criterion stays unchecked until the UI open path exists (m-5/m-6 UI tasks); no environment limitation is involved.

AC 3 (NOT checked): the snapshot history and everything a restore menu reads are implemented and proven — the_restore_list_keeps_the_last_k_autosaves_newest_first shows six autosaves pruned to K=3, listed newest first, each entry carrying label()/revision() and load()ing exactly the project it was taken from; the_history_keeps_the_newest_k_snapshots covers the store directly. There is no menu: sub-ui has no menu bar yet, so 'restored from a menu' cannot be demonstrated end to end and the criterion stays unchecked.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added sub_edit::autosave: a SnapshotStore over the project's sidecar directory (name.sub.d/autosave/) writing complete deterministic .sub snapshots via a .tmp and rename and keeping the newest K, a background Autosave worker that snapshots an EngineHandle::snapshot() Arc at most once per interval and only after a real change (off both the UI and engine threads, flushing on stop, reporting rather than raising a failed write), and check_for_recovery, which offers a snapshot newer than the project file as a Recovery with the prompt text plus recover()/discard(). Restoring is not a Command: a snapshot is a whole project, so it is handed back to be opened with a fresh engine and history. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-edit -p sub-command -p sub-model, all clean, including eight new behavioural integration tests (sidecar writes after a change, nothing when idle, many changes collapsing into one snapshot, 200 commands from an editing thread never blocked, pruning to K newest-first with every entry loading what it holds, and recovery offered, suppressed and discarded). AC 1 is checked; AC 2 and AC 3 stay unchecked because crates/sub-ui is still the eframe shell with no project-open flow and no menu, so the prompt and the restore menu cannot be shown to a user yet.
<!-- SECTION:FINAL_SUMMARY:END -->
