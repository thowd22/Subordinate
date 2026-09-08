---
id: TASK-16
title: PTS index for variable-frame-rate sources
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
priority: medium
ordinal: 37000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
VFR is a known source of proxy and seek bugs (§9); an explicit index avoids assuming constant frame duration.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Index maps frame number to PTS and keyframe flags, built lazily on first access and cached in the sidecar dir
- [ ] #2 Seek and frame stepping on the VFR fixture use the index and land on the correct frame
- [ ] #3 Index builds are cancellable background jobs
<!-- AC:END -->
