---
id: TASK-62
title: Export panel UI
status: In Progress
assignee:
  - '@opus-task-62'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 12:14'
labels:
  - ui
  - export
milestone: m-4
dependencies:
  - TASK-60
  - TASK-61
  - TASK-43
references:
  - docs/PLAN.md
priority: medium
ordinal: 83000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users pick a preset, range and output path and watch progress.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Panel offers preset, sequence, in/out range, output path and encoder override
- [ ] #2 Progress bar, ETA and cancel button wired to the export job
- [x] #3 Recent export list with open-folder action
- [x] #4 Exporter plugin presets from the exporter WIT world appear in the preset list (moved from TASK-78)
- [x] #5 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the export panel: a committed snapshot of the panel, and an interaction test that changes a setting and asserts the export request it builds
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New crates/sub-ui/src/export_panel.rs: ExportPanel state (preset list, sequence choice, range mode with in/out frames, output path, encoder override), PresetEntry/PresetSource folding sub-export PresetLibrary presets together with sub_plugin::interchange::ExportPreset entries from loaded exporter plugins.
2. ExportRequest built by the panel (preset id + source, sequence id, TimeRange, output path, encoder override) and ExportAction {Start, Cancel, ChooseOutput, OpenFolder} returned from ui(); no mutation of the project, so no Command is needed.
3. Progress: panel.apply_event(&ExportEvent) folds the export job's events into a status (idle/running/finished/cancelled/failed) with a progress bar, integer percent, ETA text and a Cancel button; a Finished event appends to the recent-export list, which offers Open folder.
4. New stable code ui.export_folder_unopenable for a reveal that fails; export panel wired into app.rs dock_ui replacing the TASK-62 placeholder label.
5. Unit tests in the module for label/ETA/request building; new crates/sub-ui/tests/export_panel.rs on the shared harness: a committed snapshot plus an interaction test that changes a setting and asserts the ExportRequest built.
6. Verify with cargo fmt, clippy -D warnings and cargo test -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented crates/sub-ui/src/export_panel.rs: ExportPanel with preset picker (library presets from sub-export PresetLibrary plus exporter-plugin presets from sub_plugin::ExportPreset in one list), sequence picker, whole-sequence or in/out range, output path with a Choose… dialog action, and an encoder override picked from sub_export::encoder_names for the selected preset's codec. A click returns an ExportAction (Start(ExportRequest), Cancel, ChooseOutput, Reveal); the panel performs nothing itself. Every range is a TimeRange of RationalTime frames at the sequence rate, never a float; the only float is the progress bar's width. Two new stable codes: ui.export_not_ready (with a field detail naming preset/sequence/output/range) and ui.export_folder_unopenable. Panel wired into app.rs in place of the TASK-62 placeholder, with presets loaded from PresetLibrary::load() and a parse failure shown in the panel rather than swallowed.

AC 2 left unchecked. The panel side is done and tested: apply_event folds a real sub_export::ExportEvent stream (Started/Progress/Finished/Cancelled/Failed) into a status with a percentage bar, an integer-arithmetic ETA line and a Cancel button that raises ExportAction::Cancel, and the recent list is appended on Finished. What is missing is below the panel: nothing yet turns the full-resolution compositor readback (TASK-58) and the offline audio mix (TASK-55) into the VideoFrameSource/AudioFrameSource that sub_export::spawn_export_job takes, so the app has no job to hand a request to and no handle to cancel. Rather than ship a button that always fails, ExportPanel::set_unavailable explains the disabled Export button and app.rs sets it (app::NO_RENDERER_REASON). That bridge needs a GPU and GStreamer encoders to verify, which this environment has neither of, and it is outside this task's criteria; it wants a task of its own.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets clean under -D warnings (no new warnings at all); cargo test -p sub-ui green — 355 lib tests (15 new in export_panel) plus tests/export_panel.rs (4 tests: committed snapshot export_panel_settings.png, the in-to-out interaction asserting the ExportRequest, the plugin preset in the list, and Open folder), and 15 doc tests. The snapshot really rendered here: this machine enumerates a wgpu adapter.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the export panel (crates/sub-ui/src/export_panel.rs) and docked it in place of the placeholder: preset picker carrying built-in, user and exporter-plugin presets in one list, sequence picker, whole-sequence or in/out range in exact RationalTime frames, output path with a file-dialog action, encoder override per the preset's codec, an export status folded from sub_export::ExportEvent with a progress bar, ETA and Cancel, and a bounded recent-export list with Open folder. Verified with cargo fmt --check, cargo clippy --workspace --all-targets (clean), 15 new module tests, 15 doc tests and a new egui_kittest suite on the shared harness: a committed snapshot (export_panel_settings.png) and an interaction test that clicks In to out and asserts the ExportRequest the Export button builds. AC 2 is only half met and left unchecked: the panel is wired to the export job's event and cancel contract, but nothing yet adapts the compositor readback and offline audio mix into the frame sources spawn_export_job needs, so the app cannot start a render and the button is disabled with a reason.
<!-- SECTION:FINAL_SUMMARY:END -->
