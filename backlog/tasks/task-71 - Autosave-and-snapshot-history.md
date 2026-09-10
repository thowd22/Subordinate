---
id: TASK-71
title: Autosave and snapshot history
status: Done
assignee:
  - '@opus-task-71'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 10:23'
labels:
  - core
  - ui
milestone: m-5
dependencies:
  - TASK-43
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
- [x] #2 On open, a newer autosave than the project file prompts to recover
- [x] #3 Snapshots keep the last K autosaves and can be restored from a menu
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the recovery prompt and the snapshot restore list: a committed snapshot of the dialog and an interaction test asserting recover and discard
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New sub-edit module `autosave`: SnapshotStore over a project's sidecar dir (name.sub -> name.sub.d/autosave/), writing deterministic sub-model JSON to a .tmp and renaming into place so a reader never sees a half file, with snapshot file names carrying the save time and engine revision.
2. Snapshot metadata (path, revision, saved_at, label) plus list() newest-first, load() back into a Project, and pruning to the last K snapshots (AC 3 data model for the restore menu).
3. Autosave worker: a thread that subscribes to the engine's ChangeEvent bus, marks the project dirty, and every N seconds writes one snapshot from an EngineHandle::snapshot() Arc. No UI thread work at all, the engine is never blocked (writes come off an Arc snapshot, not the queue), and stopping flushes a final snapshot (AC 1).
4. Recovery check on open: compare the newest snapshot against the project file's mtime/revision and return a Recovery describing the choice a UI prompt renders, with recover()/discard() (AC 2 decision logic; sub-ui is still a shell with no project-open flow, so no egui prompt is wired).
5. Stable error codes edit.autosave_failed and edit.snapshot_not_found in sub_edit::codes; no floats, no new project mutation (restore hands back a Project the caller opens).
6. Tests: unit tests in the module plus tests/autosave.rs covering interval writes after changes, pruning to K, list/load round-trip, recovery detection both ways, and that autosave never blocks the engine. Document the autosave/ sidecar layout in docs/DEVELOPMENT.md.

7. (wave 2, after TASK-43) UI half: new sub-ui module recovery.rs with RecoveryPrompt (the open-time dialog over sub_edit::autosave::check_for_recovery, stating what would be lost and offering Recover/Discard) and SnapshotMenu (the restore list, newest first, each entry loading its snapshot), both handing back a RecoveryOutcome rather than mutating the open project.

8. Wire both into SubordinateApp: open_project(path) reads a file, adopts it and raises the prompt; a File > Snapshot history menu draws the restore list; adopt_project() replaces project, sequence, compositor, viewer, scheduler and timeline, because a snapshot is a whole project and not a command.

9. Tests: crates/sub-ui/tests/autosave_recovery.rs on the shared harness in tests/support/mod.rs - interaction tests clicking Recover, Discard and a restore entry over a real sidecar directory, plus a committed egui_kittest snapshot of the dialog (AC 4).
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

2026-09-10: requeued with a dependency on TASK-43 (docking) so the recovery prompt and snapshot menu can be built once the app shell hosts panels.

2026-09-10 (wave 2, on TASK-43): the UI half.

New module crates/sub-ui/src/recovery.rs:
- RecoveryPrompt is the open-time dialog over sub_edit::autosave::check_for_recovery. open_for(path) asks the question and opens the prompt only when a snapshot is ahead of the file; the body states which autosave it is, how old it is and how much work opening the file as it stands would lose, and its two buttons are the two answers. recover() loads the snapshot (the file and the history are left untouched), discard() drops the history and keeps the file. A failed load becomes RecoveryOutcome::Failed and is shown in the dialog rather than raised, because an unreadable snapshot must not end the session. ui() draws it as a centred window; body_ui() is the same contents without one, which is what the harness lays out.
- SnapshotMenu is the restore list behind File > Snapshot history: SnapshotStore::list() read once per set_project/refresh (a menu is drawn many times a second, the history changes only when the worker writes), each entry labelled with its revision and a whole-unit age, and a click loads that snapshot. It says so when there is no project and when there are no autosaves.
- entry_label/age_text/duration_text do the label arithmetic in whole seconds. No floats, and no timeline arithmetic goes near them.

Wired into SubordinateApp: open_project(path) reads the file, adopts it, points the menu at it and raises the prompt; a File menu holds the restore list; adopt_project() replaces project, sequence, compositor, viewer, scheduler and timeline panel, because a snapshot is a whole project and not a Command. New stable code ui.project_unreadable. docs/DEVELOPMENT.md's autosave section now describes the two widgets.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui -p sub-edit all green (sub-ui 214 unit tests including 6 new ones in recovery.rs, plus 6 new integration tests in crates/sub-ui/tests/autosave_recovery.rs). This machine does enumerate a wgpu adapter, so the snapshot test really rendered and the reference PNG was recorded here.

Acceptance criteria evidence:

AC 2 (checked): recovery.rs::opening_a_project_asks_the_autosave_history_and_prompts_when_it_is_ahead drives RecoveryPrompt::open_for itself - no history opens silently, an autosave ahead of the file raises a prompt naming that revision, and answering it hands the autosaved project back and closes it. tests/autosave_recovery.rs::the_prompt_offers_the_newest_autosave_and_recovering_hands_it_back clicks Recover through the shared egui_kittest harness over a real sidecar directory and gets the newest snapshot's project, with the project file and the history untouched; a_project_saved_after_its_last_autosave_shows_no_prompt_but_still_lists_its_history proves the other direction paints no buttons. SubordinateApp::open_project is the call site.

AC 3 (checked): tests/autosave_recovery.rs::the_restore_list_shows_the_kept_autosaves_newest_first_and_restores_the_one_clicked writes six autosaves with K=3, asserts the list is revisions 6, 5, 4 newest first, clicks the middle entry by its label and gets exactly the project that snapshot held. a_menu_with_no_autosaves_says_so covers the empty list. The menu is drawn in the app's File > Snapshot history.

AC 4 (checked): crates/sub-ui/tests/autosave_recovery.rs is built on crates/sub-ui/tests/support/mod.rs. the_dialog_matches_its_snapshot commits crates/sub-ui/tests/snapshots/autosave_recovery_prompt.png (the prompt and the restore list in one frame; the wording with no project file on disk is used because it is the only shape whose text does not depend on how long the test took) and skips with a report where no adapter exists. The interaction tests assert recover and discard as required.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Autosave now has both halves. sub_edit::autosave (wave 1) keeps a history of complete .sub snapshots in the project's sidecar directory, written off the UI and engine threads at most once per interval and only after a real change, and answers whether an open should offer to recover. The new sub_ui::recovery adds the two things a user sees: RecoveryPrompt, the dialog an open raises when an autosave is ahead of the project file, stating what would be lost and offering Recover (open the snapshot, touching neither the file nor the history) or Discard (drop the history, keep the file); and SnapshotMenu, the File > Snapshot history restore list of the kept autosaves, newest first, each labelled with its revision and age and restorable with a click. Both hand a whole Project back rather than mutating the open one - a snapshot is not a Command - and SubordinateApp::open_project/adopt_project are the wiring. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-ui -p sub-edit, all clean: 6 new unit tests in recovery.rs and 6 new egui_kittest tests in crates/sub-ui/tests/autosave_recovery.rs on the shared harness, clicking Recover, Discard and a restore entry over a real sidecar directory, plus a committed snapshot of the dialog (crates/sub-ui/tests/snapshots/autosave_recovery_prompt.png). All four acceptance criteria are checked.
<!-- SECTION:FINAL_SUMMARY:END -->
