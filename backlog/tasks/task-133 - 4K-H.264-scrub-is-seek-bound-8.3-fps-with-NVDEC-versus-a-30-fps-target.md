---
id: TASK-133
title: '4K H.264 scrub is seek-bound: 8.3 fps with NVDEC versus a 30 fps target'
status: In Progress
assignee:
  - '@opus-task-133'
created_date: '2026-09-11 14:48'
updated_date: '2026-09-11 15:36'
labels:
  - media
  - performance
milestone: m-5
dependencies:
  - TASK-116
references:
  - docs/PERFORMANCE.md
priority: high
ordinal: 153000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The first hardware baseline (TASK-116, run 34611438521 on a T4 with GStreamer 1.24) measured 4K H.264 scrub at 8.284 fps through nvh264dec (p50 112 ms, p95 219 ms per seek) against 6.9 fps with software decode, while 4K playback runs at 172 fps. Scrubbing is therefore bound by the seek-to-keyframe-and-decode-forward path rather than by decoding itself. The plan's exit criterion for phase 1 is scrub above 30 fps on hardware decode. Candidates: reuse the decode-ahead ring when the target lies within the current GOP, keep a small per-GOP decoded-frame cache keyed by PTS, avoid flushing seeks for forward steps, use the PTS index to pick the nearest keyframe without a pipeline preroll, and measure seek cost separately from decode cost in perf.json.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 perf.json separates seek latency from decode-forward time per scrub step
- [ ] #2 4K H.264 scrub with hardware decode exceeds 30 fps in the hardware workflow on the NVIDIA runner, and software scrub improves proportionally on the Linux CI benchmark
- [x] #3 Frame accuracy is unchanged: the seek test suite still lands on the exact burned-in timecode
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Instrument the seek path: Decoder::seek_to records a SeekTiming (seek_nanos = flushing seek plus the first frame after it, decode_forward_nanos = the frames decoded from there to the target, frames_decoded, seeks_issued), exposed as Decoder::last_seek_timing(). Integer nanoseconds only.
2. Carry that split into perf.json: the scrub scenario collects seek and decode_forward Samples plus frames-decoded-per-step, new optional Scenario fields, and the summary line prints the split so the hardware workflow log shows where the 112 ms goes.
3. Measure locally with software decode on the 1080p and 4K fixtures to find out which half dominates.
4. Cut what the measurement says dominates, within scope: keyframe-aware seek planning driven by the existing PtsIndex (decode forward inside the GOP the decoder is already in instead of re-seeking; re-seek to the exact keyframe PTS so the overshoot backoff never runs), wired through IndexedDecoder. Pure planning logic unit-tested without hardware.
5. Verify: fmt, clippy pedantic, sub-media and subordinate-bench tests, plus a frame-accuracy run of the existing seek suite.
6. AC #2 needs the NVIDIA hardware runner, which this environment does not have; if it cannot be proven here it stays unchecked with the local software numbers recorded in notes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## What was measured

Instrumented first, then cut what the instrument showed. Decoder::seek_to now records a SeekTiming per step -- seek_nanos (the flushing seek, the demuxer's re-prime and the first frame it produces), decode_forward_nanos (the pictures from there to the target), frames_decoded and seeks_issued -- and the scrub scenario carries all four into perf.json as seek, decode_forward, frames_decoded and seeks_issued. scripts/hardware-summary.py prints the split, so the next NVIDIA run says where its 112 ms goes without anyone opening the JSON.

Local software measurement (WSL2, release, --no-gpu --seeks 20, avdec_h264), which is the shape the T4 number will have too:

| fixture | seek p50 | decode-forward p50 | pictures per step |
| --- | --- | --- | --- |
| bars_1080p | 19.6 ms | 10.1 ms | 12.7 |
| bars_2160p | 39.6 ms | 46.6 ms | 12.0 |

Neither half is the decode of the frame asked for: about 12 pictures per step are decoded only to build its reference chain, and the flush costs 20-40 ms before any picture of the new position exists.

## What was cut

The seek path is now driven by the PtsIndex when one is available (Decoder::set_index, wired automatically by IndexedDecoder and by the benchmark's scrub scenario, which builds the index before timing as the viewer does in the background).

* A forward target inside the GOP the decoder is already in decodes forward however far ahead it is, instead of guessing with the two-second forward_decode_window -- on the 250-frame-GOP fixture a five-second step went from a flush plus a re-decode of the GOP to no seek at all and fewer pictures.
* A seek that must happen aims at the keyframe the index names. The fixtures' timestamps start 80 ms in, so a target expressed in stream time could snap to the keyframe *after* the one it needed and force a second, blindly-aimed seek; that retry no longer fires.
* No step now decodes through more than the GOP its target sits in, where the time window could decode through two.

Effect on the 20-step software scrub: pictures decoded 302 -> 240 on 4K (316 -> 254 on 1080p), retry seeks gone, sustained 11.331 -> 11.712 fps (4K) and 31.358 -> 32.792 fps (1080p). The rate change is inside this machine's ~10% spread; the picture counts are exact.

## AC #2 is left unchecked

It is two claims and neither is provable here. The hardware half needs the NVIDIA runner (this machine has no GPU and no hardware decoder), and the software half asks for a proportional improvement that the numbers above do not support: the measurement says why, and it is not something this task's candidates could close. At 4K a step spends ~40 ms flushing and ~47 ms decoding a dozen pictures it will not show; 30 fps is a 33 ms budget. Getting there needs a step to stop decoding pictures it does not show (a per-GOP cache of what the step already decoded, keyframe-only decode while the playhead moves with the accurate frame drawn on release, or proxies) and the flush to stop costing tens of milliseconds. That is a change of approach rather than a tuning of this path, so it is left for the user to schedule rather than smuggled in here.

## Validation

cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-media -p subordinate-bench all green (111 lib + 61 bench unit tests and every fixture suite), including seek_fixtures, which judges a seek by the burnt-in timecode and by the whole picture -- that is the AC #3 evidence, and it is unchanged. New tests: five planner cases in decode.rs (keyframe aim, in-GOP forward step, next-GOP step, index that cannot answer, backward target) plus a SeekTiming billing test, and two fixture tests in index_fixtures.rs that prove the seek is removed on the long-GOP clip and the overshoot retry on the colour bars.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Split the scrub step into its two costs and cut the wasted seeks, but did not reach the 30 fps criterion. Decoder::seek_to now reports a SeekTiming (flushing seek versus decode-forward, pictures decoded, seeks issued) and perf.json plus the hardware job summary carry it per scrub scenario, which is AC #1; measuring with it showed a 4K step spends ~40 ms flushing and ~47 ms decoding about twelve pictures it never shows. The PtsIndex now drives seek planning (Decoder::set_index, used by IndexedDecoder and by the benchmark's scrub scenario), so a forward target inside the current GOP never flushes, a seek aims at the exact keyframe and the overshoot retry stops firing: 302 -> 240 pictures decoded over the 20-step 4K software scrub, with frame accuracy unchanged (seek_fixtures' timecode and whole-picture assertions still pass, AC #3). AC #2 is unchecked: the hardware half needs the NVIDIA runner this environment does not have, and the software half did not improve proportionally -- 11.331 -> 11.712 fps, inside the machine's noise -- because closing the rest of the gap needs a step to stop decoding pictures it does not show rather than a tuning of this path. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings, and cargo test -p sub-media -p subordinate-bench.
<!-- SECTION:FINAL_SUMMARY:END -->
