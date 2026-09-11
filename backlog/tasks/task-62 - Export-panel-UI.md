---
id: TASK-62
title: Export panel UI
status: Done
assignee:
  - '@opus-task-62'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 13:28'
labels:
  - ui
  - export
milestone: m-4
dependencies:
  - TASK-59
  - TASK-61
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
- [x] #2 Progress bar, ETA and cancel button wired to the export job
- [x] #3 Recent export list with open-folder action
- [x] #4 Exporter plugin presets from the exporter WIT world appear in the preset list (moved from TASK-78)
- [x] #5 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the export panel: a committed snapshot of the panel, and an interaction test that changes a setting and asserts the export request it builds
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New crates/sub-ui/src/export_runner.rs: the piece between the finished export panel and sub_export's job. ExportSources trait (open(request, settings) -> ExportStreams{video, audio}) so the host supplies the pixels and samples, with a blanket impl for closures; ExportRunner owning at most one ExportJobHandle.
2. ExportRunner::start resolves the request against the PresetLibrary (Preset::to_settings), applies the encoder override through EncoderPreferences, resolves ExportElements, computes frames_total by rescaling the request span to the preset's frame rate with Rounding::Ceil (exact RationalTime, no floats), builds the ExportJob and spawns it with sub_export::spawn_export_job on the app's JobService.
3. ExportRunner::poll drains ExportJobHandle::events into ExportPanel::apply_event, so the panel's progress bar, percent and ETA come from the real job; the handle is dropped on the terminal event. ExportRunner::cancel raises the job's cancel token, which is what the panel's Cancel button reaches.
4. New stable codes ui.export_busy (a second export asked for while one runs) and ui.export_preset_unsupported (a plugin preset, which no host can resolve yet); refusals surface through ExportPanel::add_problem.
5. app.rs: own a JobService and an ExportRunner, poll it once a frame, route ExportAction::Cancel to it and ExportAction::Start through it.
6. Tests: module tests for refusals and frame-count rescaling, plus crates/sub-ui/tests/export_runner.rs driving a real x264/matroska export of synthetic frames end to end - Started/Progress/Finished land in the panel with a percentage and an ETA, and a cancel run leaves the panel Cancelled with no part file.
7. Verify with cargo fmt --all --check, clippy --workspace --all-targets -D warnings and cargo test -p sub-ui under the GStreamer env.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented crates/sub-ui/src/export_panel.rs: ExportPanel with preset picker (library presets from sub-export PresetLibrary plus exporter-plugin presets from sub_plugin::ExportPreset in one list), sequence picker, whole-sequence or in/out range, output path with a Choose… dialog action, and an encoder override picked from sub_export::encoder_names for the selected preset's codec. A click returns an ExportAction (Start(ExportRequest), Cancel, ChooseOutput, Reveal); the panel performs nothing itself. Every range is a TimeRange of RationalTime frames at the sequence rate, never a float; the only float is the progress bar's width. Two new stable codes: ui.export_not_ready (with a field detail naming preset/sequence/output/range) and ui.export_folder_unopenable. Panel wired into app.rs in place of the TASK-62 placeholder, with presets loaded from PresetLibrary::load() and a parse failure shown in the panel rather than swallowed.

AC 2 left unchecked. The panel side is done and tested: apply_event folds a real sub_export::ExportEvent stream (Started/Progress/Finished/Cancelled/Failed) into a status with a percentage bar, an integer-arithmetic ETA line and a Cancel button that raises ExportAction::Cancel, and the recent list is appended on Finished. What is missing is below the panel: nothing yet turns the full-resolution compositor readback (TASK-58) and the offline audio mix (TASK-55) into the VideoFrameSource/AudioFrameSource that sub_export::spawn_export_job takes, so the app has no job to hand a request to and no handle to cancel. Rather than ship a button that always fails, ExportPanel::set_unavailable explains the disabled Export button and app.rs sets it (app::NO_RENDERER_REASON). That bridge needs a GPU and GStreamer encoders to verify, which this environment has neither of, and it is outside this task's criteria; it wants a task of its own.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets clean under -D warnings (no new warnings at all); cargo test -p sub-ui green — 355 lib tests (15 new in export_panel) plus tests/export_panel.rs (4 tests: committed snapshot export_panel_settings.png, the in-to-out interaction asserting the ExportRequest, the plugin preset in the list, and Open folder), and 15 doc tests. The snapshot really rendered here: this machine enumerates a wgpu adapter.

2026-09-11 supervisor: requeued. The export pipeline (TASK-59) and export job with progress, ETA and cancel (TASK-61) are now Done, so criterion 2 (progress bar, ETA and cancel wired to the export job) can be completed by driving the real job from the panel.

2026-09-11 (requeue): wired the panel to the real export job. New crates/sub-ui/src/export_runner.rs is the piece that was missing between them: ExportRunner::start resolves the panel's ExportRequest against the PresetLibrary (Preset::to_settings), pins the panel's encoder override through EncoderPreferences, resolves ExportElements, counts the frames by rescaling the request's span to the preset's frame rate with Rounding::Ceil (exact RationalTime, no float anywhere), and spawns the job with sub_export::spawn_export_job on the window's JobService at Priority::Normal. ExportRunner::poll drains ExportJobHandle::events into ExportPanel::apply_event once a frame, so the bar's fraction, the percentage, the frame count, the rate and the ETA are the running encoder's own numbers; ExportRunner::cancel raises the job's cancel token, which is what the panel's Cancel button reaches. At most one export runs at a time (ui.export_busy), and a plugin preset is refused rather than silently substituted (ui.export_preset_unsupported) - two new stable codes. Where the pixels come from is the host's business, handed in through the ExportSources trait, so nothing in sub-ui has an opinion about the compositor.

app.rs now holds an ExportHost (panel, preset library, runner, JobService), polls the runner once a frame, asks for a repaint every 100 ms while an export runs, routes ExportAction::Cancel to the runner and ExportAction::Start through it, and shows a refusal in the panel rather than logging it away. The one thing app.rs still cannot supply is ExportSources: nothing yet adapts the full-resolution compositor readback (TASK-58) and the offline audio mix (TASK-55) into VideoFrameSource/AudioFrameSource, so app::export_sources() returns core.unimplemented and the Export button stays disabled with app::NO_RENDERER_REASON. That bridge needs a GPU and real media and is a task of its own; the wiring it will plug into is done and tested.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui green - 368 lib tests (13 new in export_runner), 16 doc tests, and the new crates/sub-ui/tests/export_runner.rs, which encodes real 64x48 H.264/Matroska files through x264enc on this machine: one test holds the export mid-render with a gated frame source and asserts the panel is Running with a percentage, an ETA, a bar fraction in 0..=1 and a status line naming both, then lets it finish and finds the file and the recent-list entry; the other presses Cancel on a 10 000-frame export and finds the panel Cancelled, the runner free and no part-written file. Both skip themselves on a machine with no usable encoder, as the sub-export job tests do.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The export panel is complete and now drives a real export. The panel (crates/sub-ui/src/export_panel.rs) offers built-in, user and exporter-plugin presets in one list, a sequence picker, whole-sequence or in/out range in exact RationalTime frames, an output path with a file dialog, an encoder override per the preset's codec, and a bounded recent-export list with Open folder. The new crates/sub-ui/src/export_runner.rs wires it to sub_export: it turns the panel's ExportRequest into an ExportJob (preset settings, pinned encoder, frame count rescaled to the preset's rate), spawns it on the window's JobService, folds the job's event stream into the panel so the progress bar, percentage, rate and ETA are the encoder's own numbers, and hands the Cancel button the job's cancel token; where the frames come from is the host's, through the ExportSources trait. app.rs holds the panel, the library, the runner and the job pool together, polls once a frame and routes Start and Cancel. Verified with cargo fmt --check, cargo clippy --workspace --all-targets under -D warnings (clean), 368 lib tests, 16 doc tests, the egui_kittest suite (committed snapshot export_panel_settings.png plus the In-to-out interaction asserting the ExportRequest) and a new tests/export_runner.rs that encodes real H.264/Matroska files: one export is held mid-render and asserted on through the panel (percentage, ETA, bar fraction, status line), another is cancelled and leaves no part file. Still missing below the panel, and outside these criteria: nothing yet adapts the compositor readback and the offline audio mix into the job's frame sources, so the app's own Export button remains disabled with NO_RENDERER_REASON.
<!-- SECTION:FINAL_SUMMARY:END -->
