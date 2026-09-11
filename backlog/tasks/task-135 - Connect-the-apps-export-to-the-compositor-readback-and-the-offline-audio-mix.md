---
id: TASK-135
title: Connect the app's export to the compositor readback and the offline audio mix
status: Done
assignee:
  - '@opus-task-135'
created_date: '2026-09-11 19:10'
updated_date: '2026-09-11 20:00'
labels:
  - ui
  - export
  - render
milestone: m-4
dependencies:
  - TASK-62
  - TASK-58
  - TASK-55
ordinal: 155000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-62 finished the export panel and the runner that drives a real sub_export job, but nothing in the app turns the full-resolution compositor readback (TASK-58) and the offline audio mix (TASK-55) into the VideoFrameSource/AudioFrameSource that job takes, so app::export_sources() returns core.unimplemented and the Export button stays disabled behind app::NO_RENDERER_REASON. The adapters already exist, written once inside bins/subordinate-cli/src/render.rs: a per-clip decoder plus compositor readback loop, and a MixGraph compiled at the export format and rendered offline. Sharing them rather than writing a second copy is what keeps a GUI export and `subordinate-cli render` the same render.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The compositor readback and the offline audio mix are adapted into export frame sources in one shared place used by both the GUI and subordinate-cli render, not duplicated
- [x] #2 The export panel Export button is enabled in the assembled app and a GUI export writes a real file; NO_RENDERER_REASON no longer disables it
- [x] #3 The export runs off the UI thread and painting continues while it renders: nothing that decodes, composites or mixes happens on the UI thread
- [x] #4 A headless test opens a project in the assembled app on the software adapter, triggers an export with a preset and validates the written file with gst-discoverer-1.0 for frame count and audio presence
- [x] #5 A test shows a GUI export and a subordinate-cli render of the same project and preset agree: identical frame count and byte-identical audio
- [x] #6 cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and the sub-ui, sub-export and subordinate-cli test suites pass
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New crates/sub-export/src/sequence.rs behind a non-default `sequence` feature (deps sub-model, sub-media, sub-audio, sub-render, wgpu): the adapters lifted out of bins/subordinate-cli/src/render.rs, made owning so they are 'static + Send. SequenceFrames holds an Arc<Project>, a cloned Sequence, the project directory, a RenderContext clone and a Compositor, opens one Decoder per clip lazily and implements VideoFrameSource by compositing the frame at each time and reading the canvas back as RGBA. SequenceAudio implements AudioFrameSource and builds the MixGraph, decodes, folds, resamples and renders the offline mix on first read, so no decode happens on the caller's thread. settings_for_sequence(preset, sequence) is the CLI's settings rule moved across: the preset chooses container and codecs, the sequence chooses the canvas and the timebase, and a mismatch is a warning.
2. bins/subordinate-cli/src/render.rs deletes the moved code and calls the shared module, so the CLI and the GUI render through exactly the same adapters.
3. crates/sub-ui/src/export_runner.rs: settings now come from the sequence, not from the preset alone, because the compositor reads back at the sequence canvas. ExportRunner::start and settings_for take the &Project the request names its sequence in; frames_total then needs no rescale in the common case but keeps the exact Ceil rescale for a preset at another rate.
4. crates/sub-ui/src/app.rs: export_sources() becomes a real ExportSources over the app's RenderContext, the session's Arc<Project> and the open project's directory. NO_RENDERER_REASON is kept only for the case the app genuinely cannot serve - an unsaved project, whose relative media cannot be resolved - and the panel is otherwise enabled.
5. Tests: module tests in sub-export for the settings rule and the lazy audio source; a headless kittest test in sub-ui that opens a generated-fixture project in the assembled app, presses Export, runs the job to completion off the UI thread while frames keep painting, and validates the file with gst-discoverer-1.0; and an equivalence test asserting a GUI export and subordinate-cli render of the same project and preset have the same frame count and byte-identical audio.
6. Verify with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test for sub-export, sub-ui and subordinate-cli under the GStreamer env.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented. The CLI's adapters are now crates/sub-export/src/sequence.rs behind a non-default `sequence` feature (sub-audio, sub-media, sub-model, sub-render, wgpu are optional deps, so the MCP server and the plugin host still build sub-export without the GPU and decode stacks). What moved out of bins/subordinate-cli/src/render.rs, unchanged in behaviour: the per-clip decoder and compositor readback loop (now SequenceFrames, owning an Arc<Project> and Arc<Sequence> so it is 'static + Send and can be moved to a worker), the MixGraph compile-decode-resample-render pipeline (now SequenceAudio), settings_for (now settings_for_sequence, returning its warnings instead of pushing into a &mut Vec), and has_audio, sequence_frames, clips_with_effects, media_path, clip_pcm, decode_audio, to_channels, resample, trim_to_source_start and lift_render_error. The CLI now calls open_streams; nothing is duplicated.

Two behaviour changes were needed, both forced by the same fact:

1. SequenceAudio is lazy. The CLI rendered the whole mix eagerly while building the source, which is minutes of decoding for a long export; ExportSources::open is called on the UI thread, so doing that in the app would freeze the window. The mix is now rendered on the first read(), on the worker. SequenceFrames was already lazy.
2. The GUI's settings now come from the sequence, not from Preset::to_settings. The compositor reads back at the sequence canvas and nothing rescales a finished frame, so a preset's own width/height/rate cannot be what is encoded - which is what the CLI has always done. ExportRunner::start and export_runner::settings_for therefore take the &Project the request names its sequence in, and both front ends resolve through sub_export::settings_for_sequence. Without this the app would have encoded 1280x720 readbacks into a 1920x1080 pipeline.

app.rs: export_sources moved to export_runner::sequence_sources (public, so the equivalence test drives the same code the window does) and is now a real ExportSources over the window's RenderContext (Device and Queue are Send + Sync handles to the one egui device), the session's Arc<Project> and the project file's folder. NO_RENDERER_REASON no longer means 'not built yet'; it now means the one case the app genuinely cannot serve, an unsaved project, whose clips name media relative to a file that does not exist. poll_export refreshes the button's availability once a frame. New public accessors: SubordinateApp::export_panel, ::export_status and ::apply_export (the last is the path a click on Export takes).

Verification, all run on this machine under the local GStreamer env with a software (lavapipe) adapter:

- cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test --workspace green.
- crates/sub-ui/tests/export_end_to_end.rs, the new headless test: the assembled SubordinateApp, opened on a copy of examples/sample-project/demo.sub with the CC0 media scripts/get-sample-media.sh fetched, arranged for a 12-frame in-to-out export of 'Main cut' with the mezzanine preset and started with the ExportAction::Start a click on Export returns. The window painted 20 frames while the export wrote its 12, which is the 'does not block the UI thread' evidence; the report says 12 video frames; the written file probes as one 1280x720 video stream with audio beside it and a duration that rescales to exactly 12 frames at 25 fps. A second test asserts the unsaved-project refusal.
- bins/subordinate-cli/tests/gui_cli_equivalence.rs: the same project rendered twice over the generated fixtures, once by the subordinate-cli binary and once through sub_ui's own ExportPanel + ExportRunner + sequence_sources on a JobService. Both wrote 10 frames, ran the same encoder element, and their FLAC audio decodes back to bit-identical samples (compared on to_bits(), with a guard that the mix is not silence). The two .mkv files are the same size and differ only in the muxer's own header bytes.
- 6 new module tests in sub-export::sequence (canvas/timebase from the sequence, the audio codec dropped for a sequence with no clip, the empty-sequence refusal, span arithmetic, the lazy mix, and the channel-fold test moved from the CLI), plus 3 new ones in export_runner (settings equal to what the CLI resolves, the lost sequence, and the canvas assertion).

Not verified here, for the supervisor: no hardware encoder ran - x264enc and the software muxers are all this box has, so NVENC/VA-API/VideoToolbox exports through the window are untested; gst-discoverer-1.0 the *binary* is not installed in this environment, so the end-to-end test's probe went through the same GStreamer pbutils Discoverer the binary wraps (sub_media::probe), and the test additionally shells out to the binary and asserts on its report wherever it is installed, which is CI.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The editor exports. The bridge from a sequence to an encoder - the compositor's full-resolution readback for the picture and the mixer's offline render for the sound - was written once inside subordinate-cli's render subcommand; it now lives in crates/sub-export/src/sequence.rs behind a non-default `sequence` feature, and both the CLI and the editor window run through it, so a GUI export and `subordinate-cli render` of the same project and preset are the same render rather than two that resemble each other. Both sources are lazy, so ExportSources::open on the UI thread opens no decoder and mixes no sample: the work happens on the job pool, and the window paints throughout. The settings a GUI export resolves now come from the sequence's canvas and timebase, as the CLI's always have, because nothing rescales a finished frame. app::NO_RENDERER_REASON stops meaning 'not built yet' and starts meaning the one case the app cannot serve, an unsaved project whose media paths have no folder to resolve against.

Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test --workspace, all clean on a software adapter; by a new headless egui_kittest test that opens the sample project in the assembled SubordinateApp, starts a 12-frame export with the mezzanine preset through the same action the Export button returns, paints 20 frames while it runs, and probes the written file with the GStreamer discoverer for exactly 12 frames of 1280x720 video with audio beside it; and by a new equivalence test that renders one project twice, once through the subordinate-cli binary and once through sub-ui's own panel, runner and sources, and finds the same frame count, the same encoder element and bit-identical FLAC audio. No hardware encoder was exercised: this machine has only x264enc.
<!-- SECTION:FINAL_SUMMARY:END -->
