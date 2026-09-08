---
id: TASK-61
title: 'Export job with progress, ETA, cancel and error reporting'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - export
milestone: m-4
dependencies:
  - TASK-59
references:
  - docs/PLAN.md
priority: high
ordinal: 82000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Long renders need feedback and must be cancellable without leaving a broken file.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Export runs as a job with frames-done, ETA and encoder stats events
- [ ] #2 Cancel stops the pipeline and removes the partial file
- [ ] #3 Failures surface the GStreamer error with the element name
<!-- AC:END -->
