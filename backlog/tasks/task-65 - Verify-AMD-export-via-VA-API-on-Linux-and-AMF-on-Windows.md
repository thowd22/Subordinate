---
id: TASK-65
title: Verify AMD export via VA-API on Linux and AMF on Windows
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 21:35'
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

2026-09-09: AWS no longer offers any AMD GPU instance type (g4ad retired; verified via describe-instance-type-offerings across all regions). AMD parts of this task need Azure NVads V710 v5 or a user-owned AMD box; NVIDIA parts proceed on RunsOn g4dn.

2026-09-09: VA-API half verifiable on box via the hardware workflow; AMF half blocked on AMD Windows hardware.
<!-- SECTION:NOTES:END -->
