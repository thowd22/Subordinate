---
id: TASK-46
title: Audio-only file decode via symphonia
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-13
references:
  - docs/PLAN.md
priority: high
ordinal: 67000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decision-4: audio-only files skip GStreamer.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 WAV, FLAC, MP3, AAC and Ogg decode to f32 PCM with seek support
- [ ] #2 Probe reports duration and channels for audio-only items
- [ ] #3 Test decodes the WAV and FLAC fixtures bit-exactly
<!-- AC:END -->
