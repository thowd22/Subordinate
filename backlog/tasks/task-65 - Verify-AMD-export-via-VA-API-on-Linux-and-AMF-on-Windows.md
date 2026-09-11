---
id: TASK-65
title: Verify AMD export via VA-API on Linux and AMF on Windows
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
ordinal: 86000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
AMD hardware paths differ per OS and are less exercised than NVENC.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Findings and driver versions recorded in a backlog doc
- [x] #2 VA-API render on AMD Linux verified by the hardware workflow's box job (vah264enc)
- [ ] #3 AMF render on AMD Windows verified once AMD Windows hardware exists (none available; no cloud host)
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.

2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.

2026-09-09: AWS no longer offers any AMD GPU instance type (g4ad retired; verified via describe-instance-type-offerings across all regions). AMD parts of this task need Azure NVads V710 v5 or a user-owned AMD box; NVIDIA parts proceed on RunsOn g4dn.

2026-09-09: VA-API half verifiable on box via the hardware workflow; AMF half blocked on AMD Windows hardware.

2026-09-11 supervisor: VA-API on the box APU verified by hardware workflow run https://github.com/thowd22/Subordinate/actions/runs/34612380072 (vah264enc render validated by discoverer; findings recorded in the job summary and DEVELOPMENT.md). AMF half blocked on hardware.
<!-- SECTION:NOTES:END -->
