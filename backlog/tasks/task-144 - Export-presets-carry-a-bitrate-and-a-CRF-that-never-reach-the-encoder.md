---
id: TASK-144
title: Export presets carry a bitrate and a CRF that never reach the encoder
status: To Do
assignee: []
created_date: '2026-09-12 04:33'
labels:
  - export
  - bug
dependencies: []
ordinal: 164000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The export matrix (TASK-143) crosses every encoder with every preset, and the cells differ only by container and codec: youtube-1080p asks for 12000 kbit/s, mezzanine for CRF 0 and h265-archive for CRF 20, and ExportSettings - the struct the pipeline is actually built from - has no quality field at all. sub_export::pipeline never sets bitrate, qp, crf or any rate-control property on the encoder it plugs, so every export runs at the element's own default and a user who picks the mezzanine preset for a master gets the same picture as one who picked YouTube 1080p. The presets are documented in docs/user-guide.md as the way quality is chosen, so this is a silent gap between what the UI offers and what the file gets.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 ExportSettings carries the preset's video quality (average bitrate or CRF) and its audio bitrate
- [ ] #2 The pipeline sets the matching rate-control properties on each catalogued encoder, with a documented mapping per vendor (NVENC, VA, AMF, VideoToolbox, Media Foundation, x264/x265 and the AV1 encoders) and a warning rather than a failure when an element has no equivalent
- [ ] #3 A test proves two presets of different bitrates over the same source write files of materially different size
<!-- AC:END -->
