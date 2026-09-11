---
id: TASK-59
title: 'Export pipeline: appsrc video and audio to encoder and muxer'
status: Done
assignee:
  - '@opus-task-59'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 09:15'
labels:
  - export
milestone: m-4
dependencies:
  - TASK-58
  - TASK-55
  - TASK-57
  - TASK-8
references:
  - docs/PLAN.md
priority: high
ordinal: 80000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Core of phase 4: turn a sequence into a file (§5.5).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Pipeline builds appsrc (video) and appsrc (audio) into the selected encoders and an mp4, mkv or mov muxer with correct timestamps
- [x] #2 Offline audio render feeds the audio appsrc in lockstep with video
- [x] #3 Output duration equals sequence duration within one frame; audio and video are in sync in the output (verified by test with burned-in timecode and a click track)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add pipeline.rs to sub-export: Container (mp4/mkv/mov muxer), AudioCodec catalogue + probe, ExportSettings validated against the encoder probe.
2. ExportPipeline: appsrc(video RGBA, format=Time, explicit PTS/duration from the frame Rational rate) -> videoconvert -> selected encoder -> parser -> muxer -> filesink, plus appsrc(audio F32LE) -> audioconvert -> audio encoder -> muxer. All timestamp math in exact integer/Rational arithmetic, never floats.
3. Lockstep driver export(): for each video frame i push the frame, then push exactly audio_frames_through(i+1) - audio_frames_through(i) audio frames pulled from the offline audio render, padding with silence when the source ends, then EOS both appsrcs and wait on the bus.
4. Stable SubError codes for muxer/encoder/pipeline/push failures; report with frame counts and exact duration.
5. Tests: unit tests for timestamp and audio-split math, container/codec compatibility, settings validation; an integration test that exports burned-in frame-index tiles plus a per-frame click track and decodes the output back with sub-media to prove duration within one frame and A/V sync.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented crates/sub-export/src/pipeline.rs: Container (mp4mux/matroskamux/qtmux), AudioCodec (AAC/Opus/FLAC with an encoder order and parsers), ExportSettings with validation, ExportElements resolution through the TASK-57 encoder probe, and ExportPipeline building appsrc(RGBA) -> videoconvert -> encoder -> parser -> muxer -> filesink alongside appsrc(F32LE) -> audioconvert -> audioresample -> encoder -> parser -> muxer. Every timestamp is integer arithmetic on the sequence Rational: frame i starts at i*den/num seconds, and the audio a video frame owns is audio_frames_through(i+1)-audio_frames_through(i), so the two clocks cannot drift at 23.976/29.97. export_with() drives the lockstep loop (one frame, then exactly that frame's audio), pads a short mix with silence so both streams end on one timestamp, reports progress per frame and supports cancel(). New stable codes: export.invalid_settings, export.unsupported_combination, export.muxer_unavailable, export.pipeline_failed, export.push_failed, export.timeout. encoder::element_is_usable() exposes the cached READY probe for muxers, parsers and audio encoders.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-export = 28 unit + 4 integration + 2 doc tests pass against the local GStreamer 1.28 runtime (x264enc, flacenc, matroskamux, mp4mux). The integration tests in crates/sub-export/tests/export_roundtrip.rs export burned-in frame-number tiles plus a per-frame click track, then decode the file back with sub-media: every decoded frame's burned-in number matches its PTS to under one frame, the stream length is within one frame of the sequence duration, and each click lands within a tenth of a frame of its boundary. One test drives the audio appsrc from the real TASK-55 offline mixer render (MixGraphBuilder + render_audio) rather than a synthetic buffer. Hardware encoders (NVENC/VA/AMF/VideoToolbox) are not present on this machine, so only the software path was exercised here; selection for them is covered by the TASK-57 probe tests.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the export pipeline in sub-export: two appsrc branches (composited RGBA frames and the offline audio mix) feeding the probe-selected encoders and an mp4/mkv/mov muxer, with exact Rational-derived timestamps, a lockstep driver that pushes each video frame's audio span, silence padding for a short mix, per-frame progress and cancellation, and six new stable export.* error codes. Verified with cargo fmt --check, workspace clippy -D warnings and cargo test -p sub-export: four end-to-end tests export burned-in frame-number tiles with a per-frame click track and decode the result back through sub-media, proving frame numbers match their timestamps, output duration is within one frame of the sequence, and clicks land within a tenth of a frame of their boundaries; one of them feeds the audio appsrc from the real offline mixer render.
<!-- SECTION:FINAL_SUMMARY:END -->
