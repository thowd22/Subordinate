---
id: TASK-79
title: WIT analyzer world integrated with the job queue
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
  - TASK-25
references:
  - docs/PLAN.md
priority: medium
ordinal: 100000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Background analysis such as silence detection or transcripts (§6.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 analyzer world exports analyze(media-id, options) that can stream progress and returns markers, metadata or ranges
- [ ] #2 Host runs analyzers as cancellable jobs and stores results on the media item
- [ ] #3 Results can be turned into markers via a command
<!-- AC:END -->
