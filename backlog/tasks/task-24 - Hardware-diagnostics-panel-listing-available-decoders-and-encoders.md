---
id: TASK-24
title: Hardware diagnostics panel listing available decoders and encoders
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - ui
  - media
milestone: m-1
dependencies:
  - TASK-14
references:
  - docs/PLAN.md
priority: medium
ordinal: 45000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Missing GStreamer elements is the most likely support issue (§9); users and agents need to see what was detected.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Panel lists detected decoders and encoders per vendor (nvcodec, va, amf, vtenc, mf, x264) with versions
- [ ] #2 Same information is available via subordinate-cli diag as JSON
- [ ] #3 Missing expected elements show a hint with the install step from DEVELOPMENT.md
<!-- AC:END -->
