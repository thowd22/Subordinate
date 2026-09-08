---
id: TASK-13
title: Media probe with GStreamer discoverer into MediaInfo
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-10
  - TASK-3.3
references:
  - docs/PLAN.md
priority: high
ordinal: 34000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Import needs duration, streams, frame rate, VFR flag, rotation and colour metadata up front (§5.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 probe(path) returns MediaInfo with container, per-stream codec, resolution, frame rate as Rational, VFR heuristic, duration, rotation tag, colour tags, audio channels and sample rate
- [ ] #2 Probe runs with a timeout and returns a SubError for unsupported or corrupt files
- [ ] #3 Tests cover every fixture from the generator including the VFR clip
<!-- AC:END -->
