---
id: TASK-60
title: Export presets as TOML data with loader and validation
status: Done
assignee:
  - '@opus-task-60'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 09:52'
labels:
  - export
milestone: m-4
dependencies:
  - TASK-59
references:
  - docs/PLAN.md
priority: medium
ordinal: 81000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Presets are data and an extension point for plugins (§5.5).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Built-in presets: YouTube 1080p, YouTube 4K, ProRes-like mezzanine (software), H.265 archive, audio-only
- [x] #2 Preset schema: container, video codec, bitrate or CRF, resolution, frame rate, audio codec and bitrate
- [x] #3 Invalid presets fail with a SubError naming the field; user presets load from the config dir
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a presets module to sub-export: flat TOML preset schema (id, name, container, video codec/width/height/frame rate numerator+denominator, bitrate-or-CRF, audio codec/bitrate/sample rate/channels) deserialised through a raw representation so every field can be validated by name.
2. Ship the five built-in presets as data in crates/sub-export/presets/builtin.toml, embedded with include_str! and parsed by the same loader.
3. Validation: each failure is a SubError with a stable export.preset_* code and a 'field' detail naming the offending key; container/codec compatibility reuses ExportSettings::validate.
4. User presets: presets.toml in the config dir (SUBORDINATE_CONFIG_DIR override, XDG/APPDATA/Application Support otherwise), overriding built-ins by id; missing file is not an error.
5. Preset -> ExportSettings conversion, TOML round-trip, and unit tests for every built-in, each validation rule and the config-dir loader.
6. Verify with cargo fmt, clippy pedantic and cargo test -p sub-export under the GStreamer env script.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-export/presets/builtin.toml (embedded with include_str!) and crates/sub-export/src/presets.rs.

Schema, flat per [[preset]] table: id, name, container, video_codec, width, height, frame_rate_numerator, frame_rate_denominator (default 1), video_bitrate_kbps XOR video_crf, audio_codec, audio_bitrate_kbps, sample_rate (default 48000), channels (default 2). Deny-unknown-fields, and parsing goes through a raw string/integer representation so every failure can name its own field rather than surfacing a serde variant message.

Built-ins: youtube-1080p (mp4/h264 12000 kbps/aac 192), youtube-4k (mp4/h264 45000 kbps/aac 384), mezzanine (mkv/h264 CRF 0 + FLAC: the software 'ProRes-like' mezzanine, since the exporter's VideoCodec set from TASK-57/59 is h264/h265/av1 and has no ProRes encoder), h265-archive (mkv/h265 CRF 20/opus 160), audio-only (mp4/aac 320, no video table).

Validation errors are SubError with new stable codes export.preset_invalid, export.preset_unreadable and export.preset_unknown; every invalid-preset error carries a 'field' detail naming the offending key (id, name, container, video_codec, width, height, frame_rate_numerator, frame_rate_denominator, video_bitrate_kbps, video_crf, audio_bitrate_kbps, sample_rate, channels) and a 'preset' detail. Container/codec compatibility reuses Container::accepts_video/accepts_audio, so a preset can never describe a file the pipeline would refuse.

User presets: presets.toml in the config directory (SUBORDINATE_CONFIG_DIR override, otherwise XDG_CONFIG_HOME/APPDATA/Application Support, matching sub-ui's keymap and dock files). A new id is appended, a built-in's id is replaced in place; a missing file is not an error, a broken one fails with the path and the field.

Preset::to_settings() builds an ExportSettings and validates it; the audio-only preset returns export.unsupported_combination because the TASK-59 pipeline is driven by composited frames and has no video-less form. Encoder property wiring for bitrate/CRF belongs to the export driver (TASK-62/63) and was left out of scope.

Frame rates stay exact Rationals throughout: 24000/1001 round-trips, no float ever appears in the schema.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-export green (50 lib tests including 22 new preset tests, 4 integration tests, 3 doctests) under the local GStreamer env.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Export presets are now TOML data: five built-ins ship in crates/sub-export/presets/builtin.toml, embedded and parsed by the same loader and validator a user's presets.toml in the config directory goes through, so presets are an extension point rather than a hard-coded list. The schema carries container, video codec, resolution, exact rational frame rate and bitrate-or-CRF, plus audio codec, bitrate, sample rate and channels; every invalid preset fails with a SubError (export.preset_invalid / _unreadable / _unknown) whose 'field' detail names the offending key. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test -p sub-export (50 lib + 4 integration + 3 doc tests green, 22 of them new).
<!-- SECTION:FINAL_SUMMARY:END -->
