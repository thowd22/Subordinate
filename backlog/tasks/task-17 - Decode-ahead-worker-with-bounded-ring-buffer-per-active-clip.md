---
id: TASK-17
title: Decode-ahead worker with bounded ring buffer per active clip
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-15
references:
  - docs/PLAN.md
priority: high
ordinal: 38000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Playback must not decode on the compositor thread; a worker keeps a bounded number of frames ready (§4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A worker thread per active decoder keeps N frames ahead of the playhead, dropping the buffer on seek
- [ ] #2 Backpressure: the worker blocks when the ring is full and never allocates per frame
- [ ] #3 Metrics for buffer occupancy and decode time per frame are exposed via tracing
<!-- AC:END -->
