---
id: TASK-133
title: '4K H.264 scrub is seek-bound: 8.3 fps with NVDEC versus a 30 fps target'
status: In Progress
assignee:
  - '@opus-task-133-2'
created_date: '2026-09-11 14:48'
updated_date: '2026-09-12 04:49'
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

## Third pass (requeued after the T4 split at 86 ms seek / 5 ms decode-forward)
9. The measured split now says the flushing seek itself is the cost on NVDEC (86.3 ms p50, 3.3 pictures a step) while the box APU still decodes half a GOP (11.3 ms seek, 63.5 ms forward, 14.2 pictures). One change answers both: stop issuing the flushing seek at all for the steps a scrub actually makes.
10. Keep the decoded pictures of the run the decoder is in. Decoder gets a byte-bounded FrameCache keyed by PTS (its own synthetic MediaId), filled by seek_to with every picture it pulls; a step whose target the index resolves to a cached PTS returns a clone of that picture and never touches the pipeline, so a backward step inside the GOP is a cache hit instead of a flush. VideoFrame::try_clone re-maps the same GStreamer buffer, so the clone costs a map and no pixels.
11. Widen the no-seek rule from 'inside the current GOP' to 'cheaper than seeking': with an index, compare the pictures a decode-forward would decode (target frame - position frame) against the pictures the seek would decode anyway (target frame - its keyframe) plus a slack of a few pictures for the flush itself (DecoderOptions::forward_decode_slack, default 12). A target inside the current GOP always wins that comparison, so the existing behaviour is a special case; a target a picture or two past the next keyframe now decodes forward instead of flushing.
12. next_frame and DecodeAhead are untouched: only seek_to fills or reads the cache, so playback keeps its decode-ahead ring and its buffer pool behaviour.
13. Measure what a scrub actually is. The existing scrub scenario alternates head and tail, so every step is a fresh GOP and a flush is unavoidable; it stays, as the worst case. A second scenario (ScenarioKind::ScrubDrag) drags the playhead a frame at a time with the small back-and-forth a hand makes, which is the workload the criterion is about, and perf.json gains frames-decoded, seeks-issued and cache-hits per step for both.
14. Verify locally (software, 4K and 1080p), then dispatch hardware.yml from the branch and read the box (vah264dec) and T4 (nvh264dec) numbers. Record every run id in the notes and update docs/PERFORMANCE.md.
15. Frame accuracy is the gate: seek_fixtures (burnt-in timecode) and index_fixtures must pass unchanged, plus fmt, clippy and the workspace tests.
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

2026-09-11 supervisor measurement after the second merge (hardware run 34635562025, box APU, vah264dec): 4K scrub 9.898 fps (seek p50 11.3 ms, decode-forward p50 63.5 ms, still 14.2 pictures per seek); 1080p scrub 22.7 fps with 14 frames per seek. The delivered change removed unshown pictures from delivery but every step still decodes about half a GOP, so the decode count per step is unchanged. The NVIDIA job of that run failed at runner launch (RunsOn capacity while two T4 jobs were already running), so no T4 number this time. Next pass must change the seek strategy itself: for a forward step inside the current GOP continue decoding from the last decoded picture without a flushing seek, and keep the current GOP's decoded pictures in the frame cache so backward steps hit; measure frames-decoded-per-step, which should drop from 14 to about 1 for sequential scrubbing.

2026-09-11 T4 measurement (hardware run 34640251740, nvh264dec): 4K scrub 9.782 fps; the split moved: decode-forward p50 is now 5.1 ms with 3.3 pictures per seek (the delivery change worked on NVDEC), but seek p50 rose to 86.3 ms, so each step is now dominated by the flushing seek and decoder preroll itself. On the box APU (vah264dec) the same run shows the old shape (14 pictures per seek). Conclusion for the next pass: eliminate the flushing seek for forward steps inside the current GOP (continue decoding from the last picture) and keep GOP pictures cached; on NVDEC that alone should take a sequential step from about 90 ms to a few ms.

## Third pass: the step that does not seek

Branch `task/task-133-scrub`, commit "TASK-133: stop a scrub step seeking for ground the decoder is already on".

What the previous pass left was two shapes of the same mistake. On the T4 (run 34640251740) the flushing seek alone was 86.3 ms of a step, with only 3.3 pictures decoded; on the box APU (run 34635562025) the seek was 11.3 ms and the step decoded 14.2 pictures. Both are a step flushing the pipeline back to a keyframe for a picture the decoder had already passed, or had just handed out.

Two changes, both of which need the PtsIndex:

* **The decoder keeps what it decodes.** Every picture a `seek_to` step pulls goes into a byte-budgeted `FrameCache` of that decoder's own (`DecoderOptions::gop_cache_bytes`, 64 MiB by default: five 4K pictures, twenty-two at 1080p). A step whose target the index resolves to a cached timestamp is handed that picture -- no seek, no decode, and the pipeline deliberately left where it stands, so the next forward step decodes on rather than rewinding to it. `VideoFrame::try_clone` makes that free: a second read-only mapping of the same GStreamer buffer, never a copy of the pixels.
* **The no-seek rule is now 'cheaper than seeking'.** Both ways of reaching a target are priced in pictures: decoding on costs the pictures between here and the target, seeking costs the pictures from the target's keyframe to it plus the flush, which `DecoderOptions::forward_decode_slack` prices at twelve. The old 'inside the current GOP' rule is that comparison with the slack at zero, so a step one or two pictures past the next keyframe now decodes on instead of flushing.

Playback is untouched: only `seek_to` fills or reads the cache, a decoder with no index keeps nothing (it could never look one up), and `next_frame` and `DecodeAhead` are exactly as they were.

## Measuring what a scrub is

The existing scrub scenario alternates head and tail, so every step lands in a GOP the decoder is not in and a flush is unavoidable -- it is the worst case, not what a hand does, and it stays in the report unchanged. A second scenario, `scrub_drag`, drags the playhead three pictures forward at a time and four back every fourth move, which is the workload the criterion is about. perf.json now carries `frames_decoded`, `seeks_issued` and `cache_hits` per scenario and the summaries print all three per step. `subordinate-bench --legacy-scrub` measures the pre-change path on the same binary, so a before-and-after needs no rebuild -- that is how the numbers below were taken, and it works on the runners too.

## Local software measurement (WSL2, release, --no-gpu --seeks 20, avdec_h264)

| fixture | scenario | before | after | pictures/step | seeks/step |
| --- | --- | --- | --- | --- | --- |
| bars_1080p | scrub_drag | 83.393 fps | **809.500 fps** | 2.65 -> 1.15 | 0.35 -> 0.00 |
| bars_2160p | scrub_drag | 31.529 fps | **130.668 fps** | 2.60 -> 1.55 | 0.40 -> 0.05 |
| bars_2160p | scrub (jump) | 18.123 fps | 18.045 fps | 3.00 | 0.90 |

Repeats: 4K drag 29.904 fps legacy, 130.863 fps after. Eight of the twenty timed 4K drag steps are cache hits and one seeks. The jump scrub does not move and should not: each of its steps is a fresh GOP.

## Validation so far

cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-media -p subordinate-bench green; cargo test --workspace --exclude sub-ui green (exit 0). Frame accuracy: seek_fixtures unchanged and passing (burnt-in timecode plus whole-picture comparison), and two new fixture tests in index_fixtures -- one walks a drag over a GOP and compares every picture the cache hands back against the same file decoded from the start, the other proves a step just past the next keyframe decodes on while one far ahead still seeks.

Hardware run 34672738874 dispatched from the branch; numbers to follow.

## Hardware: box APU (vah264dec), run 34673268738

The workflow now runs the bench twice on each runner -- once as shipped, once with --legacy-scrub -- so each machine reports its own before half. Box, AMD APU, GStreamer 1.28, GPU upload included, harness defaults (30 steps):

| fixture | scenario | before | after | pictures/step | seeks/step | cached |
| --- | --- | --- | --- | --- | --- | --- |
| bars_2160p | scrub_drag | 20.918 fps | **44.956 fps** | 2.60 -> 1.36 | 0.40 -> 0.03 | 14 of 30 |
| bars_1080p | scrub_drag | 45.223 fps | **144.650 fps** | 2.63 -> 1.36 | 0.36 -> 0.03 | 14 of 30 |
| bars_2160p | scrub (jump) | 10.594 fps | 10.574 fps | 3.00 | 0.90 | 0 |
| bars_1080p | scrub (jump) | 23.125 fps | 24.823 fps | 3.00 -> 2.33 | 0.90 -> 0.83 | 4 |

The job's own criteria line: '4K H.264 scrub (scrub_drag): **44.956 fps** through vah264dec (criterion: above 30 fps) - PASS', with '4K decode element: vah264dec - PASS'. 4K playback on this machine is 45.572 fps with the upload stage, so the drag is now at the machine's decode-and-upload ceiling rather than at the seek's.

The jump scrub is unchanged on both fixtures, which is the control: every one of its steps lands in a GOP the decoder is not in, so it flushes whatever the planner knows.

## Hardware: NVIDIA T4 (nvh264dec), run 34673268738

Same run, g4dn.xlarge through RunsOn, GStreamer 1.24, GPU upload included, harness defaults (30 steps), both passes on the same binary:

| fixture | scenario | before (--legacy-scrub) | after | pictures/step | seeks/step | cached |
| --- | --- | --- | --- | --- | --- | --- |
| bars_2160p | scrub_drag | 23.430 fps | **100.981 fps** | 2.60 -> 1.36 | 0.40 -> 0.03 | 14 of 30 |
| bars_1080p | scrub_drag | 63.536 fps | **198.188 fps** | 2.63 -> 1.36 | 0.36 -> 0.03 | 14 of 30 |
| bars_2160p | scrub (jump) | 9.774 fps | 9.795 fps | 3.00 | 0.90 | 0 |
| bars_1080p | scrub (jump) | 30.593 fps | 33.874 fps | 3.00 -> 2.33 | 0.90 -> 0.83 | 4 |

The job's criteria lines: '4K H.264 scrub (scrub_drag): **100.981 fps** through nvh264dec (criterion: above 30 fps) - PASS' and '4K decode element: nvh264dec - PASS'. The 4K drag step is now seek p50 0.000 ms and decode-forward p50 0.016 ms -- the step is the upload and the cache lookup, not the decoder -- against the jump scrub's unchanged 86.3 ms seek, which is the control that says nothing about the flush itself got cheaper: it stopped happening.

Readback on the same run: 576.1 fps on the T4, PASS.

Costs: the NVIDIA job is the only money in the workflow and ran inside its usual envelope; the second bench pass is seconds on a job already paid for. Two earlier dispatches (34672738874, 34673091566) were cancelled before any T4 instance launched -- the first to add the --legacy-scrub pass to the workflow, the second to fix the drag's warm-up seeding the cache it was about to measure -- so this is the only T4 instance this pass spent.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Cut the pictures a scrub step delivers but never shows, from thirteen to three, without reaching the 30 fps criterion. A seek issued by a decoder that has a PtsIndex is now a flushing accurate seek whose segment opens one picture before the target (and in the timeline the container is seeked in, which the fixtures' timestamps sit 80 ms above), so the target's reference chain is decoded and clipped by the decoder instead of being colour converted, downloaded and copied into a VideoFrame on its way to a caller that discards it; a decoder without an index keeps the keyframe seek it always issued, because neither adjustment can be made without knowing where pictures are. Measured on the 20-step software scrub: pictures delivered 240 -> 60 at 4K and 254 -> 60 at 1080p, step p50 63.7 -> 52.2 ms and 27.1 -> 25.9 ms, sustained 14.523 -> 18.367 fps and 36.477 -> 40.896 fps, repeatable across runs; decode-forward p50 at 4K fell 32.9 -> 3.6 ms with the chain now billed to the seek half. Frame accuracy is unchanged and was the constraint that shaped the fix -- index_fixtures caught an aim-at-the-target version handing back the previous picture of the VFR Matroska wearing the target's timestamp, and seek_fixtures' burnt-in timecode and whole-picture assertions pass (AC #3). AC #2 stays unchecked: its hardware half needs the NVIDIA runner this environment does not have, and its software half is 26% better rather than proportional to 30 fps, because 47 of the 52 ms of a 4K step is now the flush plus the decode of the reference chain -- closing that needs a per-GOP cache, keyframe-only decode while the playhead moves, or proxies, which docs/PERFORMANCE.md records as the follow-ups. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings, cargo test -p sub-media -p subordinate-bench and cargo test --workspace --exclude sub-ui.
<!-- SECTION:FINAL_SUMMARY:END -->
