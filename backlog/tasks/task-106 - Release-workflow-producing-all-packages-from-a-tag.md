---
id: TASK-106
title: Release workflow producing all packages from a tag
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 13:41'
labels:
  - release
  - infra
milestone: m-7
dependencies:
  - TASK-103
  - TASK-104
references:
  - docs/PLAN.md
priority: medium
ordinal: 127000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Repeatable releases.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Tagging vX.Y.Z builds AppImage, Flatpak bundle, MSI and dmg and attaches them to a GitHub release
- [ ] #2 Checksums are published
- [ ] #3 A dry-run mode builds without publishing
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor: the macOS dmg (TASK-105) is back-burnered until the user's M1 Mac mini arrives; the release workflow ships AppImage/Flatpak and MSI now and gains the dmg later.
<!-- SECTION:NOTES:END -->
