---
id: TASK-66
title: Verify VideoToolbox on macOS and Media Foundation fallback on Windows
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 17:28'
labels:
  - export
  - verify
milestone: m-4
dependencies: []
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
- [ ] #2 Findings recorded in a backlog doc
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: VideoToolbox half needs the physical Mac runner; Media Foundation half runs on the Windows GPU AMI.

2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.
<!-- SECTION:NOTES:END -->
