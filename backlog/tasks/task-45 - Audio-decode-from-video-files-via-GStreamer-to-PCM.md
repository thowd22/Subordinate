---
id: TASK-45
title: Audio decode from video files via GStreamer to PCM
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-14
references:
  - docs/PLAN.md
priority: high
ordinal: 66000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decision-4: video files are demuxed once, so their audio comes from the GStreamer pipeline.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Decoder exposes an audio appsink producing interleaved f32 PCM with sample-accurate PTS
- [ ] #2 Channel layout and sample rate are reported; mono and 5.1 sources downmix to stereo with documented gains
- [ ] #3 Test decodes fixture audio and checks sample count against duration
<!-- AC:END -->
