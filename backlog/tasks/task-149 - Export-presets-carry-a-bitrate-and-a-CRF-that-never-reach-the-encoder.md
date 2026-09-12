---
id: TASK-149
title: Export presets carry a bitrate and a CRF that never reach the encoder
status: Done
assignee:
  - '@codex'
created_date: '2026-09-12 08:11'
updated_date: '2026-09-12 18:08'
labels:
  - export
  - bug
dependencies: []
ordinal: 169000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The export matrix (TASK-143) crosses every encoder with every preset, and the cells differ only by container and codec: youtube-1080p asks for 12000 kbit/s, mezzanine for CRF 0 and h265-archive for CRF 20, and ExportSettings - the struct the pipeline is actually built from - has no quality field at all. sub_export::pipeline never sets bitrate, qp, crf or any rate-control property on the encoder it plugs, so every export runs at the element own default and a user who picks the mezzanine preset for a master gets the same picture as one who picked YouTube 1080p. The presets are documented in docs/user-guide.md as the way quality is chosen, so this is a silent gap between what the UI offers and what the file gets.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 ExportSettings carries the preset video quality (average bitrate or CRF) and its audio bitrate
- [x] #2 The pipeline sets the matching rate-control properties on each catalogued encoder, with a documented mapping per vendor (NVENC, VA, AMF, VideoToolbox, Media Foundation, x264/x265 and the AV1 encoders) and a warning rather than a failure when an element has no equivalent
- [x] #3 A test proves two presets of different bitrates over the same source write files of materially different size
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Move VideoQuality next to ExportSettings (pipeline.rs) and add video_quality: Option<VideoQuality> plus audio_bitrate_kbps: Option<u32> to ExportSettings, with builders, serde defaults and validate() range checks.
2. Fill both fields in Preset::to_settings and in sequence::settings_for_sequence so every preset-driven export carries its quality.
3. Add a rate-control mapping table in encoder.rs: per catalogued element, the property knobs for average bitrate and for constant quality (NVENC, VA, AMF, VideoToolbox, Media Foundation, x264/x265, svtav1enc/av1enc/rav1enc), applied defensively (property must exist, enum nick must exist, value clamped to the pspec range) and returning warnings instead of failing when an element has no equivalent.
4. Apply the mapping in build_video_branch, and the audio bitrate (bit/s) in build_audio_branch; log warnings with tracing::warn.
5. Tests: unit tests for the table and the scaling; a preset test that quality reaches ExportSettings; an integration test that encodes the same noisy source at two very different bitrates and asserts materially different file sizes (skipped when no encoder is usable).
6. Update docs/user-guide.md wording if it claims presets choose quality; run fmt, clippy pedantic and the sub-export tests.

Resume preserved agent implementation; integrate with TASK-146 and the other export fixes on a shared export branch, then validate locally without AWS.

Post-integration review: verify vendor properties against primary GStreamer docs/source, correct per-frame NVENC QP application and VideoToolbox floating-point quality, add property-level regressions, and rerun focused quality tests and clippy.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Integrated quality mapping with TASK-146 NVENC additions: nvd3d11h264enc/nvd3d11h265enc/nvautogpuh264enc/nvautogpuh265enc now have mappings. Preserved chroma and quality together in sequence settings; fixed stale error.code() assertions. Hardware property mappings remain defensive and log unsupported properties.

Post-integration review corrected two verified vendor gaps: NVENC now applies qp-const-i/p/b individually with legacy shared-property fallback, and VideoToolbox maps CRF direction onto the real 0..1 quality property. Primary references: https://gstreamer.freedesktop.org/documentation/nvcodec/nvav1enc.html and https://github.com/GStreamer/gstreamer/blob/main/subprojects/gst-plugins-bad/sys/applemedia/vtenc.c. Two GObject property regressions prove I/P/B receive values without legacy qp-const and floating quality preserves both endpoints. 11 focused rate-control tests, 2 file-size integration tests and export clippy with warnings denied passed. CLI diag also passed all 4 tests.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Preset video quality and audio bitrate reach pipeline settings and defensive per-encoder properties, including all TASK-146 NVENC variants. Verified with the combined sub-export suite: preset propagation and catalogue mapping unit tests, actual H.264 bitrate/CRF file-size comparisons, and clippy for all sub-export targets with warnings denied. Hardware vendor execution remains for the existing matrix.
<!-- SECTION:FINAL_SUMMARY:END -->
