---
id: TASK-14
title: 'Video decoder handle: uridecodebin to appsink with hardware decode preference'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-13
  - TASK-1.3
references:
  - docs/PLAN.md
priority: high
ordinal: 35000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The preview needs a reusable per-clip decoder that yields frames with PTS and prefers nvdec, va, vtdec or d3d12 when present.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Decoder::open(path) builds a pipeline that outputs NV12 (or a documented fallback) frames with PTS as RationalTime
- [ ] #2 Decoder rank forces hardware decoders first and logs which element was chosen
- [ ] #3 Pipeline errors surface as SubError; dropping the Decoder tears the pipeline down cleanly
- [ ] #4 Integration test decodes the first 60 frames of the 1080p fixture on CI (software) and asserts monotonic PTS
<!-- AC:END -->
