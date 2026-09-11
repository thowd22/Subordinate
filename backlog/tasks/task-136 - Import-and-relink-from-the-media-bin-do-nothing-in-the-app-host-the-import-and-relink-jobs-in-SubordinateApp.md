---
id: TASK-136
title: >-
  Import and relink from the media bin do nothing in the app: host the import
  and relink jobs in SubordinateApp
status: Done
assignee:
  - '@claude'
created_date: '2026-09-11 21:01'
updated_date: '2026-09-11 22:21'
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

## Follow-up: the import and relink jobs were refused on Windows and macOS (PR #1, branch `fix/import-tests-windows`)

CI run 34649952281 was green on ubuntu-26.04 and failed on windows-latest and macos-latest with two of this task's own tests:

- `importing_from_the_bin_probes_off_the_ui_thread_and_lands_as_one_undo_step` at `media_import_app.rs:170` — "the bin shows the files as pending while they are probed"
- `relinking_from_the_bin_searches_as_a_job_and_clears_the_offline_badge` at `media_import_app.rs:396` — "the offline badge is cleared"

### Root cause: one, and it was product code, not the tests

`SubordinateApp::project_dir` canonicalises the folder holding the project file. Everything it is then compared against arrives as the OS handed it over: a file dialog's answer, an OS drop, a `scan_folder` result. `MediaPath::relative_to` compared the two with a plain textual `strip_prefix`, and the two spellings do not agree off Linux:

- **Windows** — `std::fs::canonicalize` answers with a verbatim `\\?\C:\Users\runneradmin\AppData\Local\Temp\...` path, while `temp_dir()` and the shell hand out the 8.3 short name `C:\Users\RUNNER~1\AppData\Local\Temp\...`.
- **macOS** — it resolves `/var/folders/...` to `/private/var/folders/...`, because `/var` is a symlink.

So `strip_prefix` failed on both. Every import job bounced with `model.invalid_path` on its first line, before a byte was read, and `RelinkPlan::build` rejected every match, so the item stayed offline. On Linux the two spellings already agree, which is the whole of why it passed there.

This was never only about the tests: **no import and no relink would have worked at all in the shipped v0.1.1 editor on Windows or macOS.** The 0.32 s test binary was the tell — the jobs were not probing, they were returning immediately.

That immediate return is also why line 170 was the first assertion to fail rather than one further down: a job that fails before any I/O is collected by the very next frame's `poll_media`, which clears the pending row before the test reads it.

### Fix

- `MediaPath::relative_to` compares textually first and asks the filesystem only when that fails, resolving **both** sides through `canonicalize` — which settles symlinks, 8.3 short names, `.`/`..` and the letter case the volume stores. A file not on disk yet resolves through its folder, which a relink target that has moved away needs. Resolving both sides never turns an outsider into an insider, and that is asserted.
- New `sub_model::plain_path` drops Windows' verbatim prefix, and `project_dir` uses it. `\\?\` is contagious: it leaks into the error details a user reads and into the `file://` URI GLib is asked to build, and GLib will not build one from a verbatim path.

### Test changes, kept to the timing assumption only

- The pending-rows assertion read the bin *after* a frame, but the frame that collects a finished import is the same frame that clears its pending row, so a machine quick enough to probe both fixtures inside one frame looks like a window that never showed them. It now reads before the next frame, where the answer cannot race, and checks the count rather than mere non-emptiness.
- `SubordinateApp::media_bin` refreshes the panel's status on the way out, so a caller reaching for it between frames sees the jobs as they stand rather than as they stood when the last frame started.
- Two new `sub-model` unit tests: a canonicalised project folder against a dialog-shaped path, and (Unix only) a file reached through a symlinked folder — the same mismatch the Windows runner hits, reproduced where it can be reproduced.

### Note

`the_callback_allocates_nothing_over_ten_seconds` in sub-audio was reported failing once on macOS and passed on the rerun of the same commit. Treated as a flake; not investigated further.

### CI result

PR #1, run 34652279671 (first push): **windows-latest, macos-latest and ubuntu-26.04 all green.** Confirmed from the job logs that the two tests ran rather than skipping:

- windows-latest: `importing_from_the_bin_probes_off_the_ui_thread_and_lands_as_one_undo_step ... ok`, `relinking_from_the_bin_searches_as_a_job_and_clears_the_offline_badge ... ok`
- macos-latest: both, plus `a_file_that_cannot_be_read_surfaces_a_sub_error_in_the_bin ... ok`

`the_callback_allocates_nothing_over_ten_seconds` passed on macOS in this run, which with the earlier rerun makes two clean passes against one failure — a flake, as called above.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
SubordinateApp now hosts the import and relink jobs the media bin has always been raising, so Import and Relink do something instead of logging 'the bin asked for work this window does not host yet'. Import feeds the existing ImportQueue on the window's shared JobService: the files are hashed and probed off the UI thread, the bin shows a pending row per file while that runs, and the whole gesture is applied through the session as one ImportMedia group with thumbnail and waveform jobs queued behind it. Relink opens the existing dialog, whose folder search is already a job, and its RelinkPlan is applied as one RelinkMedia group. A file that cannot be read, or an import into a project that has never been saved, surfaces as a SubError in the bin rather than a log line. OS drag-and-drop needed no new path: the bin already turns a drop into the same MediaBinAction::Import. apply_bin_action is public and exhaustive with no wildcard arm.

Verified by a new interaction test on the assembled app (crates/sub-ui/tests/media_import_app.rs): it imports the generated bars_1080p_h264.mp4 and vfr_60_30.mkv fixtures in one gesture, asserts the window painted more than one frame while probing, asserts both items carry their hash and the exact durations (125 frames at 25 fps; six seconds for the MKV), asserts one press of undo removes the whole batch, drives a real relink folder search to completion and asserts the offline badge clears and undoes, and asserts an unreadable path lands in the bin as a SubError naming the file. tests/media_import_fixtures.rs now imports the video and audio fixtures together and checks that the strip and the waveform each land in the sidecar only for the stream the file actually has. tests/media_bin.rs gained an interaction test and the media_bin_importing snapshot for the pending and failure rows. cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test -p sub-ui and cargo test -p sub-edit all pass.
<!-- SECTION:FINAL_SUMMARY:END -->
