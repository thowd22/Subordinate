---
id: TASK-35
title: 'Media bin panel: import, folders, list and grid views, metadata'
status: Done
assignee:
  - '@opus-task-35'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 16:55'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-13
  - TASK-25
  - TASK-4.4
references:
  - docs/PLAN.md
priority: high
ordinal: 56000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The project media section (§2) is the entry point for every asset.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Import via native file dialog (rfd) and drag-and-drop from the OS, running probe and thumbnails as jobs
- [x] #2 Folder tree of bins with create, rename, move; list and grid views with sortable columns (name, duration, fps, resolution)
- [x] #3 Offline items are marked and can be relinked (phase 5 completes relink)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-edit: add a MoveBin command (kind bin.move) so reparenting a bin is one undoable edit; refuse the root bin and refuse moving a bin into its own subtree; register it in register_builtin.
2. sub-ui/media_import.rs: an import pipeline on the JobService — hash the file, probe it with sub-media, convert MediaInfo into the model StreamInfo, then queue a thumbnail strip job for the imported item. ImportQueue is pumped from the UI thread and never blocks it.
3. sub-ui/media_bin.rs: the panel. Bin folder tree (create, rename, move, select), list and grid views, sortable columns (name, duration, fps, resolution) with exact rational comparisons, offline badges and a relink entry. The panel mutates nothing: it raises MediaBinAction values the host turns into sub-edit commands.
4. Import entry points: native rfd file dialog and OS drag-and-drop via egui dropped_files, both raising MediaBinAction::Import.
5. Tests: unit tests for sorting/metadata/tree helpers and the MoveBin command; headless egui frame tests (egui::Context::run_ui) driving the tree, the view toggle, column sorting and a synthesised OS file drop, applying the raised actions through the Command API and undoing them.
6. Verify: cargo fmt --all --check, clippy --workspace --all-targets -D warnings, cargo test for sub-edit and sub-ui with the GStreamer env sourced.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

- crates/sub-ui/src/media_bin.rs: the panel. Folder tree (create, rename, reparent, collapse, open), list view with sortable Name/Duration/FPS/Resolution columns, grid view, offline badge and relink entry, OS file drop and the rfd file dialog. It mutates nothing: every user action leaves as a MediaBinAction the host maps onto one sub-edit command (CreateBin, RenameBin, MoveBin, MoveToBin, RelinkMedia) or onto the import queue.
- crates/sub-ui/src/media_import.rs: ImportQueue and spawn_import_job. One job per file hashes the bytes (ContentHash), probes the streams (sub-media) and builds the MediaItem; a thumbnail strip job is queued off the back of each success. poll() is pumped from the UI thread and never blocks. MediaInfo becomes the model StreamInfo there.
- crates/sub-edit/src/commands/bin.rs: new MoveBin command (kind bin.move), needed by AC 2 - the bin set had no way to reparent a folder as one undoable edit. It refuses the root bin (edit.root_bin) and a move into its own subtree (edit.invalid_index), and its inverse restores the previous parent and index exactly. Registered in register_builtin.

Decisions

- Sorting compares exactly: durations as RationalTime (wall-clock length, so 24 frames at 23.976 sorts below 48 at 24) and frame rates by cross-multiplying, because the derived Ord on Rational would put 24 fps below 23.976. The FPS column renders the exact rational as a decimal by integer arithmetic; no float exists anywhere in the panel except egui screen coordinates.
- Moves are offered as an explicit 'Move here' button on each legal target folder rather than a drag gesture: it is keyboard- and pointer-accessible and drivable headlessly. Illegal targets (self, own subtree, current parent) show no button at all.
- MediaPath is project-relative by model rule, so importing a file outside the project folder fails with model.invalid_path and the queue reports it rather than storing an absolute path.
- The grid tile draws name and metadata over a placeholder frame. The strip its picture would come from is generated at import, but painting that JPEG needs an image decoder the UI does not carry yet; no new dependency was added for it.

Verification

- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-ui -p sub-edit: all green (15 test targets). New: crates/sub-ui/tests/media_bin.rs drives the panel headlessly through egui::Context::run_ui - a synthesised OS file drop, column-heading clicks in both directions, typing a folder name and clicking New folder/Rename, clicking Move here for an item and for a folder, the relink button, the grid toggle and the tree collapse - and applies each raised action through the Command API, undoing it. crates/sub-ui/tests/media_import_fixtures.rs imports the real bars_1080p_h264.mp4 fixture end to end: hash, probe (1920x1080, 25 fps, exactly 125 frames) and a three-picture JPEG strip in the sidecar folder, then applies and undoes ImportMedia. It ran here with SUB_FIXTURES_DIR pointed at the generated fixtures; it skips itself where they are absent.
- Not verifiable in this environment: the rfd dialog itself is a modal OS dialog and needs a desktop session, so only the code path around it (the Import action it feeds, and everything downstream) is covered by tests. Everything else in the three criteria is covered above.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the media bin panel (crates/sub-ui/src/media_bin.rs): a folder tree with create, rename and reparent, list and grid views with exactly-sorted Name/Duration/FPS/Resolution columns, offline badges and a relink entry, plus import by native rfd dialog and OS drag-and-drop. Import runs entirely as jobs in crates/sub-ui/src/media_import.rs - hash, probe, then a thumbnail strip - and the panel mutates nothing: every action maps onto one sub-edit command, including the new MoveBin command that makes reparenting a folder a single undoable edit. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test -p sub-ui -p sub-edit, including a headless egui test that drives drops, sorting, folder edits and relink through the Command API and a fixture test that imports a real 1080p25 clip and checks the JPEG strip it produced.
<!-- SECTION:FINAL_SUMMARY:END -->
