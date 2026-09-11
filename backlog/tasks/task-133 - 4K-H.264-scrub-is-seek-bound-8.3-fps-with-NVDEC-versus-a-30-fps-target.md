---
id: TASK-133
title: '4K H.264 scrub is seek-bound: 8.3 fps with NVDEC versus a 30 fps target'
status: In Progress
assignee:
  - '@opus-task-133'
created_date: '2026-09-11 14:48'
updated_date: '2026-09-11 18:42'
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
1. Second pass on the requeued task. The merged first pass instrumented the split and made the index drive seek planning; the hardware re-run (34626676058) showed decode-forward dominates (72.4 ms p50 at 4K, 14.2 pictures decoded per step), so the cut has to be the pictures a step decodes but never shows.
2. Root cause: every picture between the keyframe and the target is pushed through videoconvert, downloaded out of the decoder's memory and copied into a VideoFrame (12 MB per 4K picture) only to be thrown away, because the seek uses KEY_UNIT|SNAP_BEFORE, which moves the segment start back to the keyframe so nothing downstream is out of segment.
3. Fix: seek accurately at the target itself (FLUSH|ACCURATE, no KEY_UNIT). The demuxer still starts at the preceding keyframe, but the segment starts at the target, so GstVideoDecoder clips the reference-chain pictures and they never reach videoconvert, the appsink or a VideoFrame copy. Frame accuracy is unchanged: the step still returns the first frame at or after the target.
4. Adjust the overshoot retry so it cannot misfire on an accurate seek: with a PtsIndex, an overshoot means the delivered picture is later than the frame the index says the target needs; without one, keep the legacy rule.
5. Keep the index-driven DecodeForward decision (no seek at all when the target is ahead inside the GOP the decoder is already in).
6. Measure locally with software decode on the 1080p and 4K fixtures before and after; record the split, pictures decoded and fps in the notes.
7. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, cargo test -p sub-media -p subordinate-bench including the frame-accurate seek fixture suite (AC #3 evidence).
8. AC #2's hardware half still needs the NVIDIA runner this environment does not have; check it only if the software half proves out and the hardware half can be argued, otherwise leave it unchecked with the numbers in notes.
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

2026-09-11 supervisor measurement after the merge (hardware run 34626676058): 4K scrub 9.161 fps on the T4 (seek p50 31.7 ms, decode-forward p50 72.4 ms, 14.2 pictures decoded per seek) and 9.945 fps on the box APU (seek p50 11.3 ms, decode-forward p50 61.5 ms, 14.2 pictures per seek). The instrumentation (criterion 1) is in; the speed-up is not. The split shows decode-forward dominates: every scrub step re-decodes about half a GOP from the keyframe. Direction for the next pass: when consecutive scrub targets fall inside the same GOP and move forward, continue decoding from the last decoded picture instead of seeking to the keyframe again (the decoder already holds the reference state), and keep the decoded pictures of the current GOP in the frame cache so backward steps within the GOP are cache hits. A forward sweep should then cost one decode per step, which is the 30 fps target. Requeued.

## Second pass (requeued): the pictures a step delivers but never shows

The re-run after the first pass (34626676058) showed decode-forward dominating: 72.4 ms p50 at 4K for 14.2 pictures a step. Those pictures are the target frame's reference chain -- they have to be *decoded*, which is the codec's business, but nothing needed them *delivered*. They were, because a KEY_UNIT seek asks the demuxer to move the segment back to the keyframe, which puts every picture from there inside the segment: colour converted, downloaded out of the decoder's memory and copied into a VideoFrame (12 MB apiece at 4K) on the way to a caller that throws them away.

A seek is now a flushing accurate seek (FLUSH|ACCURATE, no KEY_UNIT) whose segment opens just before the target. The demuxer still starts the decoder at the keyframe before it -- it cannot decode from anywhere else -- and the decoder clips the chain instead of pushing it.

Two details the fixtures forced, both in Decoder::seek_aim / accurate_aim:
* The aim is the picture *before* the target, not the target. A decoder trims a buffer that straddles the start of the segment rather than dropping it, so on the VFR Matroska fixture (whose blocks all carry the same default duration, which is not their real one) aiming at the target handed back the previous picture wearing the target's timestamp -- right PTS, wrong picture. Opening a picture early makes any trimmed buffer one the step discards anyway. index_fixtures caught this.
* The aim is moved into the timeline the container is seeked in, which is the presentation timestamps shifted by the first picture's own (80 ms on these fixtures). Without that the segment opens *after* the frame asked for and the decoder clips the very picture the caller wants -- at the end of a file that means no frame at all.

Both need to know where pictures are without decoding, so the accurate seek is only used when a PtsIndex is set; a decoder without one keeps exactly the keyframe seek it always issued (SeekMode::Keyframe). The viewer and the benchmark both index, so both get the fast path.

Also fixed on the way: a seek that ends the stream without ever delivering a picture used to surface as media.no_video_stream rather than as end-of-stream, and a seek whose segment opened past the last picture now gets one attempt further back (bounded to one, and skipped entirely when the index says the target really is past the end).

## Measured, same machine, release, --no-gpu --seeks 20, avdec_h264

| fixture | pictures delivered over 20 steps | step p50 | sustained |
| --- | --- | --- | --- |
| bars_1080p before | 254 | 27.1 ms | 36.477 fps |
| bars_1080p after | 60 | 25.9 ms | 40.896 fps (40.244 on a repeat) |
| bars_2160p before | 240 | 63.7 ms | 14.523 fps |
| bars_2160p after | 60 | 52.2 ms | 18.367 fps (18.709 on a repeat) |

decode_forward p50 at 4K went 32.9 ms -> 3.6 ms; the reference chain now sits in the seek half (34.2 -> 47.2 ms) because it is decoded before the first delivered picture exists. Three pictures a step instead of thirteen, on both fixtures. The rate change is well outside this machine's run-to-run spread this time, and the picture counts are exact.

## AC #2 stays unchecked

The hardware half needs the NVIDIA runner this environment does not have. The software half improved by 26% at 4K and 12% at 1080p, which is real but is not 30 fps: 47 of the 52 ms of a 4K step is now the flush plus the decode of the reference chain, and 30 fps is a 33 ms budget. Closing that needs a step to stop decoding the chain at all -- a per-GOP cache so a step inside the GOP the last one landed in is a cache hit, keyframe-only decode while the playhead moves with the accurate frame drawn on release, or proxies -- rather than another tuning of this path. docs/PERFORMANCE.md records the numbers and names those follow-ups.

## Validation

cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-media -p subordinate-bench all green, including seek_fixtures (which judges a seek by the burnt-in timecode and by the whole picture, the AC #3 evidence) and index_fixtures. New unit tests: the accurate aim opens a picture early, names its instant in the container's timeline and declines when the index cannot answer; the seek flags say which kind of seek was asked for; only an index can say whether a target is inside the file.

Validation (second pass): cargo fmt --all --check clean, cargo clippy --workspace --all-targets -- -D warnings clean, cargo test -p sub-media -p subordinate-bench green, and cargo test --workspace --exclude sub-ui green (sub-ui's snapshot suite was not run here; it does not exercise the seek path directly). cargo doc -p sub-media --no-deps raises nothing new. AC #1 and AC #3 stay checked: AC #1's split is still reported per scrub scenario (its halves now bill the reference chain to the seek, which the harness prints), and AC #3's evidence is seek_fixtures, which judges every seek by the burnt-in timecode and by the whole picture and still passes -- the VFR regression this pass found and fixed is exactly that gate doing its job. AC #2 remains unchecked.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Cut the pictures a scrub step delivers but never shows, from thirteen to three, without reaching the 30 fps criterion. A seek issued by a decoder that has a PtsIndex is now a flushing accurate seek whose segment opens one picture before the target (and in the timeline the container is seeked in, which the fixtures' timestamps sit 80 ms above), so the target's reference chain is decoded and clipped by the decoder instead of being colour converted, downloaded and copied into a VideoFrame on its way to a caller that discards it; a decoder without an index keeps the keyframe seek it always issued, because neither adjustment can be made without knowing where pictures are. Measured on the 20-step software scrub: pictures delivered 240 -> 60 at 4K and 254 -> 60 at 1080p, step p50 63.7 -> 52.2 ms and 27.1 -> 25.9 ms, sustained 14.523 -> 18.367 fps and 36.477 -> 40.896 fps, repeatable across runs; decode-forward p50 at 4K fell 32.9 -> 3.6 ms with the chain now billed to the seek half. Frame accuracy is unchanged and was the constraint that shaped the fix -- index_fixtures caught an aim-at-the-target version handing back the previous picture of the VFR Matroska wearing the target's timestamp, and seek_fixtures' burnt-in timecode and whole-picture assertions pass (AC #3). AC #2 stays unchecked: its hardware half needs the NVIDIA runner this environment does not have, and its software half is 26% better rather than proportional to 30 fps, because 47 of the 52 ms of a 4K step is now the flush plus the decode of the reference chain -- closing that needs a per-GOP cache, keyframe-only decode while the playhead moves, or proxies, which docs/PERFORMANCE.md records as the follow-ups. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings, cargo test -p sub-media -p subordinate-bench and cargo test --workspace --exclude sub-ui.
<!-- SECTION:FINAL_SUMMARY:END -->
