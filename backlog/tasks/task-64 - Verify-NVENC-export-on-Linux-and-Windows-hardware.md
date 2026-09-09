---
id: TASK-64
title: Verify NVENC export on Linux and Windows hardware
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
ordinal: 85000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Phase 4 exit criterion requires real hardware verification; CI runners have no GPUs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A documented manual run of cli-render with nvh264enc and nvh265enc on an NVIDIA machine on Linux and on Windows
- [ ] #2 Output validated with ffprobe or discoverer and visually checked
- [ ] #3 Findings, driver versions and caveats recorded in a backlog doc
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.

2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.
<!-- SECTION:NOTES:END -->
