---
id: TASK-144
title: >-
  Wire the viewer and playback to the decoder so the app shows decoded video
  while scrubbing and playing
status: To Do
assignee: []
created_date: '2026-09-12 05:04'
labels:
  - ui
  - media
  - render
  - bug
milestone: m-1
dependencies:
  - TASK-133
  - TASK-56
  - TASK-22
  - TASK-70
priority: high
ordinal: 164000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-133's agent found that nothing in sub-ui or sub-edit calls sub_media::Decoder: the viewer panel draws the compositor texture but no decoded frames feed it, so in the shipped app scrubbing and playback show no video from real media (the only production caller of the seek path is sub-export). The decode pipeline exists and is fast (decoder with hardware preference, decode-ahead ring, frame cache, PTS index, and since TASK-133 a GOP cache giving 45 to 101 fps 4K scrub on real GPUs) but the UI never constructs it. The compositor (TASK-21/56) samples clip frames through an interface that must be backed by per-clip IndexedDecoders driven by the playhead and the playback scheduler (TASK-23/56), off the UI thread, with proxies (TASK-70) when enabled.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Opening the sample project and scrubbing the timeline shows the correct decoded frame in the viewer and the pop-out; the Xvfb window smoke and the Linux desktop smoke screenshots show real picture content (not black) and the desktop smoke asserts it by comparing against a frame rendered by subordinate-cli
- [ ] #2 Play/JKL playback decodes ahead on worker threads with the audio clock as master; dropped frames are counted; the UI thread never blocks on decode
- [ ] #3 Scrubbing uses Decoder::set_index / IndexedDecoder so the GOP cache applies; the hardware workflow's scrub_drag number is measured through the same code path the viewer uses
- [ ] #4 kittest interaction test: scrub to frame N shows the frame whose burned-in timecode is N on a generated fixture
<!-- AC:END -->
