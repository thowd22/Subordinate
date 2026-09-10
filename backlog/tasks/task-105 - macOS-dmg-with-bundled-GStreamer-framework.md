---
id: TASK-105
title: macOS dmg with bundled GStreamer framework
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 00:03'
labels:
  - release
milestone: m-7
dependencies:
  - TASK-66
references:
  - docs/PLAN.md
priority: low
ordinal: 126000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Completes the three-OS promise.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 dmg contains an app bundle with the GStreamer framework and vtenc plugin
- [ ] #2 Launch and export verified on a clean macOS machine; Gatekeeper notes documented
- [ ] #3 Universal or Apple Silicon build decision recorded
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-10: macOS dmg packaging is back-burnered until the user's M1 Mac mini arrives; CI on macos-latest still builds and tests the software path.
<!-- SECTION:NOTES:END -->
