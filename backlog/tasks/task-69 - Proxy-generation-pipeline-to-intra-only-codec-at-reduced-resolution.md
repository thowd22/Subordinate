---
id: TASK-69
title: Proxy generation pipeline to intra-only codec at reduced resolution
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - media
milestone: m-5
dependencies:
  - TASK-16
  - TASK-25
  - TASK-57
references:
  - docs/PLAN.md
priority: high
ordinal: 90000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Long-GOP sources scrub badly; proxies fix it (§5.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Job transcodes a media item to DNxHR LB or MJPEG at half or quarter resolution into the sidecar dir, using hardware decode when available
- [ ] #2 VFR sources use the PTS index so proxy frames map one-to-one to originals
- [ ] #3 Auto-generation triggers for sources above a configurable resolution threshold
<!-- AC:END -->
