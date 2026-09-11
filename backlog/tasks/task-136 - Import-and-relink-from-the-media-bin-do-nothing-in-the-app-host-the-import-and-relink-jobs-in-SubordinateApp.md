---
id: TASK-136
title: >-
  Import and relink from the media bin do nothing in the app: host the import
  and relink jobs in SubordinateApp
status: Done
assignee:
  - '@claude'
created_date: '2026-09-11 21:01'
updated_date: '2026-09-11 21:32'
labels:
  - ui
  - media
  - bug
milestone: m-2
dependencies:
  - TASK-127
  - TASK-35
priority: high
ordinal: 156000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
User report on the v0.1.0 MSI: choosing Import in the media bin logs 'the bin asked for work this window does not host yet: Import { paths: [...mkv], bin: BinId(...) }' and nothing happens. SubordinateApp::apply_bin_action only applies actions that map to a single command; Import and Relink are jobs (content hash, probe via sub-media, then ImportMedia/RelinkMedia commands, then thumbnail and waveform jobs) and the window has no host for them (crates/sub-ui/src/app.rs, apply_bin_action). The CLI and MCP paths that probe media exist (TASK-94's Services in bins/subordinate-cli/src/host.rs; sub-media probe and hashing; the job service from TASK-25). The GUI must run the same pipeline off the UI thread, show progress in the bin, apply the resulting commands through the session (one undo entry per import batch), and kick off thumbnails and waveforms. Drag-and-drop from the OS onto the bin must go through the same path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Import from the bin's Import button, from a native file dialog selection and from an OS drag-and-drop probes the files off the UI thread, adds MediaItems with hash and stream info through the Command API as one undo step, and starts thumbnail and waveform jobs; the bin shows the items with a pending state until probing finishes
- [x] #2 Relink from the bin runs the relink search as a job and applies RelinkMedia; offline items clear their badge
- [x] #3 An interaction test on the assembled app imports a generated fixture (and an MKV, since the report was an MKV) and asserts the media item appears with correct duration; an unreadable path surfaces a SubError in the bin rather than a log line
- [x] #4 No remaining 'does not host yet' branch in app.rs; every MediaBinAction is handled
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. media_import.rs: give ImportQueue batches (ImportBatch id from submit; poll returns one FinishedImport per batch, only once every import in it has finished) so one import gesture is one undo step. Queue a waveform job beside the thumbnail strip for items with audio. Expose the in-flight source paths for the bin's pending state.
2. media_bin.rs: add a BinStatus the host sets each frame (pending import file names, and the SubErrors the last imports/relinks reported) and draw it: a pending row per file still probing, and the failures as code + message rather than a log line. into_command is unchanged; no new action variants.
3. app.rs: move the JobService out of ExportHost onto SubordinateApp, since imports, hashes, waveforms and exports are documented as sharing it. Add a MediaHost holding the ImportQueue (rebuilt when the project folder changes), the RelinkDialog and the problems the bin shows. apply_bin_action becomes public, exhaustive and non-logging: Import feeds the queue, Relink/RelinkAll open the dialog. A per-frame poll_media applies each finished batch through session.apply_group as one undo step, and the relink dialog is drawn as a window whose RelinkPlan is applied as one group too. Drag-and-drop already arrives as MediaBinAction::Import, so it takes the same path.
4. Tests: extend crates/sub-ui/tests/media_bin.rs with the pending/problem drawing (interaction + snapshot); new crates/sub-ui/tests/media_import_app.rs driving the assembled SubordinateApp — import the generated mp4 fixture and the vfr_60_30.mkv fixture, assert the items land with the right duration and one undo step each, and that an unreadable path shows a SubError in the bin. Update media_import_fixtures.rs and the unit tests for the new poll shape.
5. cargo fmt --all --check, clippy -D warnings, sub-ui and sub-edit suites.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Everything the import needed already existed and was simply never hosted: crates/sub-ui/src/media_import.rs (ImportQueue, hashing, probe, thumbnail jobs), crates/sub-ui/src/relink_dialog.rs (the folder search as a job) and sub-edit's ImportMedia/RelinkMedia and RelinkPlan. The bug was one branch in SubordinateApp::apply_bin_action that logged anything that was not a single command. No new probing, hashing or search code was written.

Changes:
- media_import.rs: ImportQueue now hands out an ImportBatch per submit and poll returns one FinishedImport per batch, released only once every file in it has finished; that is what makes one Import gesture one history group. queue_thumbnails became queue_followups and also spawns sub_media::spawn_waveform_job for a file that has audio (a strip only for one that has picture). pending_paths() exposes what is still probing for the bin to draw.
- media_bin.rs: new BinStatus (files being probed, SubErrors from the last import/relink) set by the host each frame and drawn above the folders in both view modes. The panel still owns no jobs and starts none.
- app.rs: the JobService moved out of ExportHost onto SubordinateApp, since imports, hashes, thumbnails, waveforms, relink searches and exports are documented as sharing it. New MediaHost holds the ImportQueue (rebuilt when the project folder moves), the RelinkDialog and the problem list. apply_bin_action is public and exhaustive with no wildcard arm, so a variant added later fails to compile rather than being dropped. poll_media() drains batches into session.apply_group(IMPORT_GROUP_LABEL, ...) and relink_ui() applies the RelinkPlan as one group under sub_edit::relink::RELINK_GROUP_LABEL.

Two deliberate decisions worth recording:
- An import into a project that has never been saved is refused with the new ui.import_not_ready code and shown in the bin, because a media path is stored relative to the project file. This is the same rule that holds the Export button closed.
- The pending row is a label, not an egui::Spinner: a spinner asks egui to repaint for ever, which breaks Harness::run and burns a core in the real window. The window already asks for its own next frame while a job is in flight.

Validation (WSL, software Vulkan, GStreamer from the local gstroot, SUB_FIXTURES_DIR=/home/admin2/Subordinate/fixtures):
- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-ui: 373 unit + every integration suite green, including the new tests/media_import_app.rs (3 tests) and the extended tests/media_bin.rs (14) and tests/media_import_fixtures.rs.
- cargo test -p sub-edit: green.
- New snapshot crates/sub-ui/tests/snapshots/media_bin_importing.png recorded with UPDATE_SNAPSHOTS=1 and eyeballed: two 'Importing... <file>' rows and one '[model.invalid_path] ...' line above the folder tree.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
SubordinateApp now hosts the import and relink jobs the media bin has always been raising, so Import and Relink do something instead of logging 'the bin asked for work this window does not host yet'. Import feeds the existing ImportQueue on the window's shared JobService: the files are hashed and probed off the UI thread, the bin shows a pending row per file while that runs, and the whole gesture is applied through the session as one ImportMedia group with thumbnail and waveform jobs queued behind it. Relink opens the existing dialog, whose folder search is already a job, and its RelinkPlan is applied as one RelinkMedia group. A file that cannot be read, or an import into a project that has never been saved, surfaces as a SubError in the bin rather than a log line. OS drag-and-drop needed no new path: the bin already turns a drop into the same MediaBinAction::Import. apply_bin_action is public and exhaustive with no wildcard arm.

Verified by a new interaction test on the assembled app (crates/sub-ui/tests/media_import_app.rs): it imports the generated bars_1080p_h264.mp4 and vfr_60_30.mkv fixtures in one gesture, asserts the window painted more than one frame while probing, asserts both items carry their hash and the exact durations (125 frames at 25 fps; six seconds for the MKV), asserts one press of undo removes the whole batch, drives a real relink folder search to completion and asserts the offline badge clears and undoes, and asserts an unreadable path lands in the bin as a SubError naming the file. tests/media_import_fixtures.rs now imports the video and audio fixtures together and checks that the strip and the waveform each land in the sidecar only for the stream the file actually has. tests/media_bin.rs gained an interaction test and the media_bin_importing snapshot for the pending and failure rows. cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test -p sub-ui and cargo test -p sub-edit all pass.
<!-- SECTION:FINAL_SUMMARY:END -->
