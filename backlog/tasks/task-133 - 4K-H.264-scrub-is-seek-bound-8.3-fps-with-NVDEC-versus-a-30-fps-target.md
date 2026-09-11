---
id: TASK-133
title: '4K H.264 scrub is seek-bound: 8.3 fps with NVDEC versus a 30 fps target'
status: To Do
assignee: []
created_date: '2026-09-11 14:48'
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
- [ ] #1 perf.json separates seek latency from decode-forward time per scrub step
- [ ] #2 4K H.264 scrub with hardware decode exceeds 30 fps in the hardware workflow on the NVIDIA runner, and software scrub improves proportionally on the Linux CI benchmark
- [ ] #3 Frame accuracy is unchanged: the seek test suite still lands on the exact burned-in timecode
<!-- AC:END -->
