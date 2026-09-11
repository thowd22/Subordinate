---
id: TASK-64
title: Verify NVENC export on Linux and Windows hardware
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 15:03'
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
- [ ] #1 Output validated with ffprobe or discoverer and visually checked
- [x] #2 Findings, driver versions and caveats recorded in a backlog doc
- [x] #3 NVENC render on Linux verified by the hardware workflow's NVIDIA job (nvh264enc; nvh265enc where the runner exposes it)
- [ ] #4 NVENC render on Windows verified once the NVIDIA Windows AMI (TASK-115) exists
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.

2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.

2026-09-11 supervisor: Linux NVENC verified by hardware workflow run https://github.com/thowd22/Subordinate/actions/runs/34612380072 (nvh264enc render validated by discoverer). Windows half depends on TASK-115.
<!-- SECTION:NOTES:END -->
