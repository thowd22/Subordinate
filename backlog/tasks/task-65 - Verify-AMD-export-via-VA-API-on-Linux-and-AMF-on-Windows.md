---
id: TASK-65
title: Verify AMD export via VA-API on Linux and AMF on Windows
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 17:29'
labels:
  - export
  - verify
milestone: m-4
dependencies:
  - TASK-116
references:
  - docs/PLAN.md
priority: medium
ordinal: 86000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
AMD hardware paths differ per OS and are less exercised than NVENC.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cli-render with vah264enc on an AMD Linux machine and amfh264enc on an AMD Windows machine both produce valid files
- [ ] #2 Findings and driver versions recorded in a backlog doc
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.

2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.
<!-- SECTION:NOTES:END -->
