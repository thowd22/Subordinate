---
id: TASK-104
title: Windows MSI installer with GStreamer runtime
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - release
milestone: m-7
dependencies:
  - TASK-64
  - TASK-66
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
