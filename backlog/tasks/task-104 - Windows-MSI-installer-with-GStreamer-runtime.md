---
id: TASK-104
title: Windows MSI installer with GStreamer runtime
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 13:41'
labels:
  - release
milestone: m-7
dependencies:
  - TASK-59
  - TASK-63
references:
  - docs/PLAN.md
priority: high
ordinal: 125000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Windows users expect a single installer.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cargo-wix (or equivalent) builds an MSI bundling the GStreamer MSVC runtime with amf, nvcodec and mf plugins
- [ ] #2 Install, launch, export and uninstall verified on a clean Windows VM
- [ ] #3 Code-signing steps documented even if unsigned for MVP
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor: dependencies changed from TASK-64/66 to the export pipeline and CLI render; NVIDIA-on-Windows verification is TASK-115's job once the AMI exists.
<!-- SECTION:NOTES:END -->
