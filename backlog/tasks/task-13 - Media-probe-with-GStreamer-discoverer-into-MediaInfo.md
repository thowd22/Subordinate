---
id: TASK-13
title: Media probe with GStreamer discoverer into MediaInfo
status: Done
assignee:
  - '@opus-task-13'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 02:24'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-10
  - TASK-3.3
references:
  - docs/PLAN.md
priority: high
ordinal: 34000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Import needs duration, streams, frame rate, VFR flag, rotation and colour metadata up front (§5.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 probe(path) returns MediaInfo with container, per-stream codec, resolution, frame rate as Rational, VFR heuristic, duration, rotation tag, colour tags, audio channels and sample rate
- [x] #2 Probe runs with a timeout and returns a SubError for unsupported or corrupt files
- [x] #3 Tests cover every fixture from the generator including the VFR clip
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add gstreamer-pbutils/gstreamer-video deps to sub-media and a media.* error code registry in sub_core terms.
2. probe(path)/probe_with(path, ProbeOptions) -> MediaInfo: container media type and container-format tag, duration as RationalTime at the nanosecond rate, seekable, per-stream codec + human codec description, resolution, frame rate and pixel aspect as Rational, rotation/mirror from the image-orientation tag, colour tags parsed from caps colorimetry into sub_model ColorTags, audio channels, sample rate and language.
3. VFR heuristic: a parse-only (never decode) filesrc ! parsebin scan collects video buffer PTS, sorts them (decode order != presentation order) and compares shortest to longest frame gap with a 5 percent tolerance -> FrameTiming Constant/Variable/Unknown.
4. Timeout: one budget for the whole probe, handed to the discoverer and then to the scan; discoverer results and GError domains map to media.probe_timeout, media.unsupported, media.file_unreadable, media.probe_failed.
5. Tests: unit tests for the classifier, orientation tags, colorimetry mapping and nanosecond durations, plus an integration test probing every generated fixture (including the VFR clip) against the manifest, and error-path tests for missing, non-file and corrupt inputs.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation
- New crates/sub-media/src/probe.rs: probe(path) and probe_with(path, ProbeOptions) return MediaInfo { container media type, container_format tag, duration, seekable, video[], audio[] }. VideoStreamInfo carries codec media type plus GStreamer's human codec description, width, height, frame_rate: Option<Rational>, sample_aspect: Rational, Rotation (clockwise) + mirrored from the image-orientation tag, ColorTags parsed from caps colorimetry, interlaced and FrameTiming. AudioStreamInfo carries codec, channels, sample_rate and language. Durations are RationalTime at the exact nanosecond rate (probe::NANOSECONDS); no value anywhere is a float.
- Colour tags reuse sub_model::sequence::ColorTags so the probe fills the model's own vocabulary; the mapping goes through gstreamer_video::VideoColorimetry so shortcut strings (bt709, bt2100-pq) and full range:matrix:transfer:primaries strings both parse.
- VFR heuristic: a parse-only pass (filesrc ! parsebin, buffer pad probes, fakesinks, never a decoder) collects video PTS, sorts them because a long-GOP stream is parsed in decode order, drops duplicates and compares the shortest gap with the longest against a 5 percent tolerance. Result is FrameTiming::Constant / Variable / Unknown; anything that goes wrong in the scan degrades to Unknown rather than failing an otherwise successful probe. ProbeOptions.scan_frame_timing turns it off.
- Timeout: ProbeOptions.timeout is one budget for the whole probe; the discoverer takes it (clamped up to the one second GStreamer's discoverer insists on) and the scan gets what is left. Error codes are new media.* constants in sub_media::codes: init_failed, file_unreadable, probe_failed, unsupported, probe_timeout, each SubError carrying the path in details.

Verification (all run in this sandbox)
- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-media -p sub-test-support: 10 unit + 7 integration + doc tests pass.
- Fixtures were generated with scripts/gen-fixtures.sh --long, so all seven fixtures (including the VFR clip and the ten-minute long-GOP clip with B-frames) were really probed: resolution, frame rate, duration within 1 ms of the manifest, Rec.709 tags, no rotation, and frame timing matching the manifest's vfr flag, with vfr_60_30.mkv the only variable one.

Environment note: this machine has the GStreamer 1.24 runtime and plugins but no -dev packages, so building needed libgstreamer-plugins-base1.0-dev extracted into the scratchpad sysroot, and generating fixtures needed gstreamer1.0-tools plus the x264, videoparsers, timecode and libav plugin files. Nothing in the repo depends on that arrangement; CI installs the pinned runtime and dev files per TASK-1.3.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the GStreamer discoverer probe to sub-media: probe(path)/probe_with(path, ProbeOptions) return a MediaInfo with container, per-stream codec and description, resolution, frame rate and pixel aspect as exact Rationals, duration as a nanosecond RationalTime, rotation and mirror from the orientation tag, colour tags in the model's own ColorTags vocabulary, audio channels, sample rate and language, plus a FrameTiming verdict from a parse-only PTS scan that never decodes. The probe runs under one timeout budget and reports stable media.* SubError codes for missing, non-file, unsupported, corrupt and timed-out inputs. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-media against every generated fixture, including the VFR clip (the only one classified Variable) and the ten-minute long-GOP clip.
<!-- SECTION:FINAL_SUMMARY:END -->
