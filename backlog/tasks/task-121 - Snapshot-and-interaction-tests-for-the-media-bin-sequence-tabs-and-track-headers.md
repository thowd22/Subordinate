---
id: TASK-121
title: >-
  Snapshot and interaction tests for the media bin, sequence tabs and track
  headers
status: Done
assignee:
  - '@opus-task-121'
created_date: '2026-09-09 18:21'
updated_date: '2026-09-10 08:34'
labels:
  - ui
  - test
milestone: m-2
dependencies:
  - TASK-119
  - TASK-35
  - TASK-33
  - TASK-34
priority: medium
ordinal: 141000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
These panels are already merged without UI tests. Lock in their layouts and the undoable actions they expose.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Snapshots: bin in list and grid view with the sample project, sequence tab strip with three sequences, track headers with a muted and a locked track
- [x] #2 Interaction tests: rename a bin, switch sequence tab, toggle mute and lock; each asserts state through the Command API and undo restores it
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Extend the sub-ui kittest suites (media_bin.rs, sequence_tabs.rs, track_headers.rs) with the shared support harness (mod support).
2. Snapshots: media bin in list view and in grid view over the committed sample project; the sequence tab strip with three sequences; the track header column with a muted track and a locked track.
3. Interaction tests driven through AccessKit: rename a bin (RenameBin command applied through History, undo restores the old name); switch sequence tab (SequenceTabAction::Switch applied through SequenceTabs::switch_to, switching back restores the remembered view state - tab switching is view state, not a Command); toggle mute and toggle lock (TrackAction::into_command applied through History, undo restores the flag).
4. Verify with cargo fmt --check, clippy pedantic -D warnings and cargo test -p sub-ui; record snapshots only if this machine enumerates a wgpu adapter, otherwise report that they must be recorded on CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added to the existing sub-ui suites (each now pulls in the shared egui_kittest harness with 'mod support;').

Snapshots (recorded on this machine's lavapipe adapter, the same backend CI records on, all inside the committed size budget):
- media_bin_list.png: the bin in list view over the sample project's root folder, showing the metadata columns and the offline item drawn in the offline colour.
- media_bin_grid.png: the bin in grid view with 'Interviews' open, so the breadcrumb, the 'Move here' target and a tile with duration and resolution are all in frame.
- sequence_tab_strip.png: the strip with three sequences (the sample project's Main and Titles plus an added Inserts) and the new-sequence button.
- track_headers_muted_and_locked.png: the header column for the sample sequence with track 1 muted and track 2 locked, both toggles drawn selected.

Interaction tests (AccessKit, no adapter needed):
- media_bin.rs renaming_a_folder_through_the_harness_applies_and_undoes_a_command: clicks the rename field, types, clicks 'Rename'; the raised MediaBinAction::RenameBin is applied with sub_edit::commands::RenameBin through History and History::undo puts the old name back.
- track_headers.rs the_harness_toggles_mute_and_lock_through_the_command_api: clicks 'M' then 'L'; each TrackAction goes through into_command and History::apply_boxed, and two undos clear both flags.
- sequence_tabs.rs clicking_a_tab_through_the_harness_switches_and_switches_back: clicks 'Titles' then 'Main'. Switching a tab is view state, not a project mutation, so no Command exists to undo; what stands in for undo is switching back, and the test asserts the remembered zoom, scroll, lane scroll and playhead all come back exactly.

Observation, not fixed here (out of this task's scope): the bin's folder tree draws its rows left to right rather than stacked, because MediaBinPanel::tree runs inside the allocate_ui of a horizontal_top and inherits its layout. The snapshots record the panel as it is today; worth a follow-up if that is unintended.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui all green (251 tests across the crate, including the four new snapshots and three new interaction tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Locked the media bin, the sequence tab strip and the track header column down with four committed egui_kittest snapshots (bin list, bin grid, three-sequence tab strip, headers with a muted and a locked track) and three AccessKit interaction tests that rename a folder, switch sequence tabs and toggle mute and lock. The bin rename and the two track toggles are applied through the Command API and undone in the test; a tab switch is view state rather than a Command, so that test asserts the remembered view is restored on switching back. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-ui, all clean.
<!-- SECTION:FINAL_SUMMARY:END -->
