---
id: TASK-55
title: Offline audio render through the mixer for export
status: Done
assignee:
  - '@opus-task-55'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 19:00'
labels:
  - audio
  - export
milestone: m-3
dependencies:
  - TASK-48
references:
  - docs/PLAN.md
priority: high
ordinal: 76000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Export must produce identical mixes to playback without real-time constraints.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 render_audio(sequence, range) produces interleaved PCM deterministically using the same mixer code
- [x] #2 Two renders of the same project are bit-identical
- [x] #3 Test compares a rendered fade against expected gain curve
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-audio/src/offline.rs: ClipSource trait (sample_rate, channels, seek, read), PcmSource over decoded Pcm, OfflineSequence (MixGraph + one source per clip slot + block size).
2. render_audio(sequence, range) converts the TimeRange to exact frames at the sequence rate, builds a real Mixer via mixer(), installs a ring per slot, seeks each source to its clip-relative offset, then tops up the rings and calls Mixer::process block by block so export uses the identical mixing code as playback.
3. Expose MixGraph::slot_span for the clip start/length behind a slot; keep frames_at shared.
4. Tests: bit-identical repeat renders, fade-in/fade-out compared against the expected linear gain curve, sub-range rendering with mid-clip seek, gain/mute/solo agreement with Mixer::process, channel/rate mismatch errors.
5. Verify with cargo fmt, clippy -D warnings and cargo test -p sub-audio.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-audio/src/offline.rs. render_audio(sequence, range) builds a real Mixer over the sequence's published MixGraph, installs one PcmReader ring per clip slot exactly as playback does, seeks each source to its clip-relative offset and drives Mixer::process block by block, so export runs the identical mixing code (same fades, clip/track/master gains, mute and solo) with the rings topped up ahead of every block instead of by a real-time worker. ClipSource is the offline counterpart of a playback ring (sample_rate, channels, seek, read); PcmSource covers already-decoded PCM. Sources must already run at the sequence rate and channel count, matching the ring contract, so resampling stays in crate::resample. Missing or short sources render as silence and are logged as underruns rather than failing the render.

Supporting changes: MixGraph::slot_span exposes the clip behind a slot, mixer::frames_at is pub(crate) so the range conversion is the same RationalTime -> frame conversion the graph uses (no float timeline math), and its GRAPH_INVALID messages now name the field ('clip start', 'render range start') instead of hard-coding 'clip'.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (workspace build with the local GStreamer prefix); cargo test -p sub-audio -> 48 lib + 5 fixture + 1 no-alloc + 4 doc tests pass. New tests cover bit-identical repeat renders (to_bits comparison), the fade curve against the expected linear amplitude ramp, equality with a hand-driven playback Mixer, sub-range renders as exact slices, block-length independence, mute/solo, silent and short sources, empty and negative ranges, and source layout rejection.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Offline audio render for export lands in sub-audio as offline::render_audio(sequence, range), which mixes through a real Mixer over the sequence's published MixGraph rather than a parallel code path: one PCM ring per clip slot, sources seeked to their clip-relative offset, rings filled ahead of each Mixer::process block. Verified by tests that two renders are bit-identical, that a render equals a hand-driven playback mixer sample for sample, and that a rendered fade follows the expected linear gain curve; cargo fmt, workspace clippy -D warnings and cargo test -p sub-audio all pass.
<!-- SECTION:FINAL_SUMMARY:END -->
