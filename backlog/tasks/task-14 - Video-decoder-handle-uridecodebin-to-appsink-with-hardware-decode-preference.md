---
id: TASK-14
title: 'Video decoder handle: uridecodebin to appsink with hardware decode preference'
status: Done
assignee:
  - '@opus-task-14'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 03:20'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-13
  - TASK-1.3
references:
  - docs/PLAN.md
priority: high
ordinal: 35000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The preview needs a reusable per-clip decoder that yields frames with PTS and prefers nvdec, va, vtdec or d3d12 when present.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Decoder::open(path) builds a pipeline that outputs NV12 (or a documented fallback) frames with PTS as RationalTime
- [x] #2 Decoder rank forces hardware decoders first and logs which element was chosen
- [x] #3 Pipeline errors surface as SubError; dropping the Decoder tears the pipeline down cleanly
- [x] #4 Integration test decodes the first 60 frames of the 1080p fixture on CI (software) and asserts monotonic PTS
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add gstreamer-app dependency to sub-media and a new decode module.
2. Decoder::open/open_with builds uridecodebin -> videoconvert -> appsink with NV12 system-memory caps; frames carry PTS/duration as RationalTime in nanoseconds.
3. Hardware preference: bump registry ranks of nvdec/va/vtdec/d3d12/qsv/amf/msdk video decoder factories above software once per process, and log the decoder element decodebin actually plugged (deep-element-added).
4. Errors: new media.* codes (decode_failed, decode_timeout, no_video_stream); Drop sets the pipeline to Null.
5. Tests: unit tests for hardware-name classification, error codes and unreadable paths; integration test decode_fixtures.rs decodes the first 60 frames of bars_1080p_h264.mp4 asserting monotonic PTS, skipping when fixtures are absent.
6. Verify fmt, clippy, cargo test (GStreamer plugins beyond core are unavailable locally, so runtime decode tests skip here).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation: crates/sub-media/src/decode.rs adds Decoder (uridecodebin -> videoconvert -> appsink), VideoFrame, DecoderOptions, FrameFormat and HardwarePreference; three new stable codes in sub-media's codes module (media.decode_failed, media.decode_timeout, media.no_video_stream). Dependency added: gstreamer-app 0.25.

Decisions:
- Sink caps name exactly one format (NV12 by default, I420 as the documented fallback via DecoderOptions::format). A caps list let videoconvert keep the decoder's own layout, so the same file decoded to I420 on one machine and NV12 on another; naming one format makes the output predictable.
- uridecodebin gets expose-all-streams=false and caps=video/x-raw(ANY) so only video is exposed and hardware memory is still autoplugged; a second video pad is sent to a fakesink rather than left dangling.
- Hardware preference is registry rank: every video decoder factory whose element name starts with a known hardware family (nv*, va*, vtdec, d3d11/12, qsv, amf, msdk) is set to Rank::PRIMARY+64 once per process, which is what decodebin's autoplug sorts on. The plugged decoder is logged at INFO via deep-element-added and exposed as Decoder::decoder_element().
- next_frame slices its wait (100 ms) and reads the bus between slices, so a pipeline error surfaces at once instead of after the whole frame budget; that also cut the decode test suite from 10 s to under 1 s.
- Drop sets the pipeline to Null and logs rather than panicking.

Verification (this machine has no GPU, so decoding is software; GStreamer 1.24 plus base/good/bad/ugly/libav plugins were extracted into a user-local sysroot and scripts/gen-fixtures.sh was run so the fixture tests really execute):
- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test --workspace: all green; sub-media 19 unit + 9 decode integration + 7 probe integration + 2 doc tests.
- AC1: decode_fixtures.rs decodes bars_1080p_h264.mp4 and asserts NV12, 1920x1080, plane count and strides, and PTS as RationalTime at the nanosecond rate.
- AC2: decode.rs test ranking_a_decoder_family_makes_decodebin_plug_it promotes openh264dec past the default avdec_h264 through the same promote_decoders() path used for hardware and asserts decodebin then plugs openh264dec; the plugged element is logged (observed 'video decoder plugged decoder=avdec_h264 hardware=false'). No GPU decoder exists here, so the hardware families themselves are covered by the name-classification tests, not by real hardware.
- AC3: missing path, directory, corrupt file and audio-only file all return media.* SubErrors; dropping a decoder mid-stream eight times in a row keeps re-opening successfully.
- AC4: the first 60 frames of the 1080p fixture decode with strictly increasing PTS and exact 40 ms spacing.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added sub_media::Decoder: a per-clip uridecodebin -> videoconvert -> appsink pipeline that yields NV12 (or I420 on request) frames whose PTS are exact RationalTime nanoseconds, prefers hardware decoders by raising their registry rank and logs the element decodebin plugs, reports failures as media.* SubErrors and tears its pipeline down on drop. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test --workspace against generated fixtures (software decode; 60-frame monotonic-PTS integration test plus a rank-promotion test that proves autoplug follows the forced rank).
<!-- SECTION:FINAL_SUMMARY:END -->
