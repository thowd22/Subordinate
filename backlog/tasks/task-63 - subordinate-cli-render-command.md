---
id: TASK-63
title: subordinate-cli render command
status: In Progress
assignee:
  - '@opus-task-63'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 12:09'
labels:
  - cli
  - export
milestone: m-4
dependencies:
  - TASK-60
  - TASK-61
  - TASK-6
references:
  - docs/PLAN.md
priority: high
ordinal: 84000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Headless render is the CI smoke test and what agents call (§5.5).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 subordinate-cli render project.sub --sequence NAME --preset NAME --out PATH renders with progress on stderr and exit code 0 on success
- [x] #2 --encoder overrides selection; --range trims
- [ ] #3 CI renders the sample project with x264 on all three OSes and validates the output with the discoverer
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a render subcommand to subordinate-cli: 'render <project> --sequence NAME --preset NAME --out PATH [--encoder NAME] [--range IN:OUT] [--verify]'; usage text and parser tests.
2. New bins/subordinate-cli/src/render.rs: load the project, resolve the sequence by name (or the only one), resolve the preset from PresetLibrary::load, build ExportSettings with the preset's container/codecs/audio format and the sequence's own canvas and exact frame rate, apply --encoder as an EncoderPreferences override and --range as an exact frame range at the sequence timebase.
3. Video: a VideoFrameSource that, per frame, resolves the layers under the playhead (sub_render::resolve_layers_at), decodes each clip's source frame through sub_media::Decoder in NV12, uploads via Nv12Converter, composites with Compositor and reads the canvas back as RGBA. One decoder cached per media item.
4. Audio: build a MixGraph from the sequence's audio tracks (track gain/mute/solo, clip gain and fades), decode and resample each clip's audio to the sequence rate as a PcmSource, render offline with sub_audio::offline::render_audio and push the PCM as the export's audio. No audio track, or a preset with no audio codec, means a video-only export.
5. Drive it through sub_export::ExportJob so progress, ETA and encoder stats are reported: human-readable progress lines on stderr, the final report as JSON on stdout, exit 0 on success.
6. --verify probes the written file with the GStreamer discoverer (sub_media::probe) and fails when the output carries no video stream or the wrong duration.
7. Tests: parser and range unit tests; an integration test that builds a project over the generated fixtures and renders it with x264, skipping when no wgpu adapter or fixtures exist.
8. CI: add a headless render of examples/sample-project/demo.sub with x264 on all three OSes, validated by --verify.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Starting: no project->frames or project->mix-graph wiring existed before this task; the render command is where they are first assembled.

Implemented and verified locally on Linux (WSL, software Vulkan via lavapipe, workspace GStreamer with x264/FLAC).

What was built:
- bins/subordinate-cli/src/render.rs: the whole headless render path, assembled here for the first time. Picture: per frame the layers under the playhead are resolved with sub_render::resolve_layers_at, each layer's source frame decoded at its exact source time (one Decoder per clip, kept open so seek_to decodes forward inside its GOP window), uploaded through Nv12Converter and composited, then read back as RGBA. Sound: the sequence's audio tracks are compiled into a MixGraph (track gain/mute/solo, clip gain and fades), each clip's audio decoded (GStreamer for files with video, symphonia otherwise), channel-adapted and resampled to the export format, and rendered offline through sub_audio::offline::render_audio over the exact sample span the video frames cover (ExportSettings::audio_frames_through). Encoding: sub_export::ExportJob, so progress, ETA and encoder stats are reported and a failed render deletes the part-written file.
- main.rs: 'render <project> --preset <id> --out <file> [--sequence <name>] [--encoder <element>] [--range IN:OUT] [--verify]', usage text and parser tests.
- --verify probes the written file with the GStreamer discoverer (sub_media::probe) and fails when it carries no video stream.
- .github/workflows/ci.yml: a 'Headless render smoke test' step on all three OSes rendering one second of examples/sample-project/demo.sub with --encoder x264enc and --verify.

Decisions:
- The preset chooses container, codecs and audio format; the sequence chooses the canvas and the exact timebase, because the compositor draws at the sequence resolution and nothing rescales a finished frame. A preset that asks for another canvas or rate is reported in a 'warnings' array rather than written into the caps.
- The mix graph runs at the export sample rate and channel count, so clip audio is resampled once on the way in rather than twice.
- Clip plugin effects are NOT run by a headless render yet (that needs the wasm plugin host bound to the compositor's EffectSource); every clip carrying an enabled effect is named in 'warnings'. Follow-up work, outside this task's criteria.

Verification run here:
- cargo fmt --all --check: clean. cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p subordinate-cli: 58 unit + all integration tests pass, including the three new tests in bins/subordinate-cli/tests/render.rs (whole-sequence render with progress on stderr and exit 0; --range 10:16 trimming to six frames with the mixer's audio in the file; four failure paths reporting core.not_found, export.preset_unknown, export.unknown_encoder and core.invalid_argument as JSON SubErrors with no file left behind).
- The exact CI command was run against the real sample project after scripts/get-sample-media.sh: 25 frames of 'Main cut' at 1280x720 25 fps, x264enc + FLAC in Matroska, one second of audio (48000 audio frames), discoverer reporting video/x-h264 1280x720 and audio/x-flac 48 kHz stereo, exit 0, about one second of wall clock.

AC #3 is left unchecked: the CI step is written and proven on Linux here, but this environment cannot run GitHub Actions on Windows and macOS, so 'CI renders the sample project on all three OSes' is unproven until the branch is pushed and the matrix goes green.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
subordinate-cli grew a 'render' subcommand that draws a sequence through the GPU compositor, mixes its audio offline and encodes both through the export job: progress on stderr, a JSON report on stdout, exit 0 on success, --encoder pinning the encoder element, --range trimming an exact frame span and --verify probing the written file with the discoverer. Verified with cargo fmt/clippy (clean), cargo test -p subordinate-cli (all pass, including three new end-to-end tests that render real fixture media with x264 and read the file back), and by running the CI command itself against examples/sample-project/demo.sub: 25 frames of h264 plus FLAC in Matroska, confirmed by the discoverer. AC #3 stays unchecked because the three-OS CI matrix cannot be run from this environment; the workflow step is in place and proven on Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
