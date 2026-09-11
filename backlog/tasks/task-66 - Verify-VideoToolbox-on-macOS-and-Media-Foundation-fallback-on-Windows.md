---
id: TASK-66
title: Verify VideoToolbox on macOS and Media Foundation fallback on Windows
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 16:03'
labels:
  - export
  - verify
milestone: m-4
dependencies:
  - TASK-117
  - TASK-115
references:
  - docs/PLAN.md
priority: medium
ordinal: 87000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Completes the cross-OS encoder matrix.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cli-render with vtenc_h264 on macOS and mfh264enc on a Windows machine without vendor encoders produce valid files
- [x] #2 Findings recorded in a backlog doc
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: VideoToolbox half needs the physical Mac runner; Media Foundation half runs on the Windows GPU AMI.

2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.

2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.

2026-09-09: VideoToolbox hardware half is blocked on a user-owned Mac (TASK-117); no cloud Mac will be used. Media Foundation half proceeds on the Windows GPU AMI.

2026-09-10: VideoToolbox half waits for the user's incoming M1 Mac mini (TASK-117).

2026-09-11 supervisor: Media Foundation half verified on the Windows GPU runner by TASK-115 run 34618546433 (mfh264enc present on Windows Server 2022 with the T4). VideoToolbox half still waits for the user's M1 Mac mini (TASK-117).
<!-- SECTION:NOTES:END -->
