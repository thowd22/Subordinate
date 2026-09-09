---
id: TASK-15
title: 'Frame-accurate seek: keyframe seek then decode-forward'
status: Done
assignee:
  - '@opus-task-15'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 04:42'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-14
references:
  - docs/PLAN.md
priority: high
ordinal: 36000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Scrubbing and split-at-playhead demand exact frames on long-GOP sources; naive seeks land on keyframes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 seek_to(time) performs a flushing keyframe-backward seek then discards frames until PTS >= target
- [x] #2 Seeking within the current GOP forward does not re-seek
- [x] #3 Test suite asserts the burned-in timecode in fixtures matches the requested frame for 50 random targets including the last frame
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add Decoder::seek_to(target: RationalTime) to crates/sub-media/src/decode.rs: a flushing keyframe-backward seek (FLUSH|KEY_UNIT|SNAP_BEFORE) followed by decoding forward, discarding frames until PTS >= target; returns the landed frame or None past the end.
2. Track the decoder position (PTS of the last delivered frame) and a seek counter; add a pure plan_seek() decision fn so a forward target within the GOP window decodes forward instead of re-seeking (no new seek issued), while a backward or far-forward target re-seeks.
3. Add DecoderOptions::forward_decode_window (GOP-sized budget) and the stable error code media.seek_failed; keep all timing in RationalTime (ns), never floats.
4. Unit-test plan_seek without GStreamer; integration-test against the generated fixtures: identify each frame by the hash of the burned-in timecode band (proved unique per frame and distinct from the static picture area) and assert 50 deterministic pseudo-random seek targets, the first frame and the last frame all land on exactly the requested frame.
5. Verify with cargo fmt --check, clippy -D warnings and cargo test for sub-media.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation (crates/sub-media/src/decode.rs):
- Decoder::seek_to(RationalTime) issues a flushing keyframe-backward seek (FLUSH | KEY_UNIT | SNAP_BEFORE) and then decodes forward, discarding frames until PTS >= target; returns the landed frame, or None past the end. Comparisons are exact RationalTime, so a target counted in frames matches a nanosecond timestamp with no float and no tolerance.
- Decoder::position() and Decoder::seek_count() expose the playhead and the number of flushing seeks issued; the pure plan_seek() decides between decoding forward and re-seeking.
- DecoderOptions::forward_decode_window (default 2 s) is the GOP budget: a target ahead of the position and inside it is reached by decoding forward, so no seek is issued; anything behind the position, or further ahead, re-seeks.
- The first seek waits for the pipeline to pre-roll (a seek sent earlier is dropped), and a flushing seek clears end-of-stream so a decoder that ran to the end is seekable again.
- Overshoot retry: the fixture's timestamps start 80 ms in, so a seek in stream time can land on the keyframe *after* the target (reproduced at frames 24/49/74/99). When the first frame after a seek is past the target, the decoder aims further back (250 ms, doubling, capped at 8 s, clamped at the stream start) and decodes forward again.
- New stable error code media.seek_failed for a refused seek or a pipeline that never becomes seekable.

Verification (fixtures generated locally with scripts/gen-fixtures.sh against a sandboxed GStreamer 1.24.2, software decode):
- cargo fmt --all --check: clean. cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-media: 26 + 9 + 7 + 7 unit/integration tests and 2 doctests pass, including the new crates/sub-media/tests/seek_fixtures.rs (7 tests).

AC3, exactly what is proven: the sweep seeks to 50 deterministic pseudo-random targets (seeded xorshift), including frame 0 and the last frame 124, half of them at the frame's own timestamp and half at an instant one nanosecond inside the previous frame's interval. Each landed frame is checked two ways: (1) its burnt-in timecode is read out of the luma plane and must name the requested frame's second - the digits are learnt from a sequential decode where frame n must show the timecode of frame n, so the learning pass fails if the burn-in does not follow the timecode; (2) its whole picture must be bit-for-bit the picture the fixture decodes at that index sequentially, the 125 pictures being pairwise distinct, which pins the exact frame. The frames counter of the burn-in cannot be thresholded: timeoverlay paints it over the noise field on the right of the SMPTE pattern, so a per-digit OCR of that field is not possible with this fixture. Frame-level identity is therefore carried by the exact picture comparison rather than by reading the two frames digits.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added Decoder::seek_to to sub-media: a flushing keyframe-backward seek followed by decoding forward until the presentation timestamp reaches the target, with exact RationalTime comparisons, a GOP-sized forward-decode window that skips the seek for a nearby forward target, an overshoot retry for containers whose timestamps do not start at zero, and the stable media.seek_failed code. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-media, including a new fixture test that seeks to 50 deterministic random targets (frame 0 and the last frame among them) and checks each landed frame against the burnt-in timecode read from its pixels and against a bit-exact comparison with the sequentially decoded picture.
<!-- SECTION:FINAL_SUMMARY:END -->
