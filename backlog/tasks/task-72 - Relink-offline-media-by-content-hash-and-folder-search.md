---
id: TASK-72
title: Relink offline media by content hash and folder search
status: Done
assignee:
  - '@opus-task-72'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 18:34'
labels:
  - ui
  - core
milestone: m-5
dependencies:
  - TASK-35
  - TASK-3.3
references:
  - docs/PLAN.md
priority: medium
ordinal: 93000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Projects move; relinking must be quick and safe.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Relink dialog lets the user pick a file or a folder to search recursively; matches by content hash first, then by name
- [x] #2 Bulk relink applies to all offline items and is one undo step
- [x] #3 A relinked item updates its relative path and clears the offline flag
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-edit: new relink module — bounded recursive folder scan, match offline items by content hash first then by file name, build a RelinkPlan of RelinkMedia commands (rejecting files outside the project folder), and apply the plan through History::begin_group/commit_group so a bulk relink is one undo step.
2. sub-ui: RelinkDialog panel — pick a single file (rfd) or a folder to search; the folder scan and hashing run on the JobService so the UI thread never blocks; the dialog reports matches and raises the plan for the host to apply.
3. media_bin: add a 'Relink all' toolbar action for the offline items so bulk relink is reachable, keeping the existing per-item Relink action.
4. Tests: unit tests in sub-edit for scan depth, hash-before-name matching, one-file-one-item, rejection of out-of-project files, and one-undo-step round trip; headless sub-ui tests for the dialog and the new bin action.
5. Verify with cargo fmt, clippy -D warnings and cargo test for sub-edit and sub-ui (sub-ui needs the GStreamer env script).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Search and plan live in the new sub-edit relink module (crates/sub-edit/src/relink.rs): a bounded, breadth-first folder walk that never descends symlinked folders, hash-then-name matching where each candidate file is claimed by at most one item, and a RelinkPlan that turns matches into ordinary RelinkMedia commands. Bulk relink needed no new command kind: History::begin_group/commit_group already collapses several commands into one undo step, which is the existing convention, so RelinkPlan::apply wraps the relinks in a group labelled 'Relink media'.

Files outside the project folder cannot be stored (MediaPath is project-relative by model rule), so such a match is refused with model.invalid_path and reported in RelinkPlan::rejected, which the dialog names on screen rather than dropping silently.

The dialog (crates/sub-ui/src/relink_dialog.rs) keeps the UI thread free: the folder walk and every content hash run as one JobService job, and the dialog is pumped once a frame. The media bin gained a 'Relink all' control and MediaBinAction::RelinkAll beside the existing per-item Relink.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-edit -p sub-ui all green, including the new crates/sub-edit/tests/relink.rs (7 tests over real files on disk) and crates/sub-ui/tests/relink_dialog.rs (4 headless egui tests, run three times for flakiness).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added relinking of offline media by content hash and folder search. sub-edit gained a relink module: a bounded recursive folder scan, matching that tries the content hash first and the file name second (case-insensitively, one file per item), and a RelinkPlan that applies the resulting RelinkMedia commands inside a history group so a bulk relink is one undo step and rejects files outside the project folder. sub-ui gained a RelinkDialog that offers 'Choose file...' and 'Search folder...', runs the walk and the hashing on the JobService so the UI thread never blocks, and reports what was found by hash, by name, or not at all; the media bin now offers 'Relink all' for every offline item. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test -p sub-edit -p sub-ui, including 7 new file-backed tests in crates/sub-edit/tests/relink.rs and 4 headless egui tests in crates/sub-ui/tests/relink_dialog.rs that prove hash-before-name matching, the relinked path and cleared offline flag, and one-press undo of a two-item bulk relink.
<!-- SECTION:FINAL_SUMMARY:END -->
