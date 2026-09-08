---
id: TASK-26
title: Benchmark harness for scrub and playback performance
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - test
  - media
milestone: m-1
dependencies:
  - TASK-23
references:
  - docs/PLAN.md
priority: medium
ordinal: 47000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Phase 1 exit criteria are numeric; a repeatable harness prevents regressions.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A criterion or custom benchmark measures decode-to-texture latency and sustained scrub fps on the 1080p and 4K fixtures
- [ ] #2 Results are written as JSON and summarised in CI logs on Linux
- [ ] #3 Baseline numbers are recorded in docs/PERFORMANCE.md
<!-- AC:END -->
