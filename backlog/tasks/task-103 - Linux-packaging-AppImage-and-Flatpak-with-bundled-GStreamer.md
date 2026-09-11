---
id: TASK-103
title: 'Linux packaging: AppImage and Flatpak with bundled GStreamer'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 13:41'
labels:
  - release
milestone: m-7
dependencies:
  - TASK-59
  - TASK-67
  - TASK-63
references:
  - docs/PLAN.md
priority: high
ordinal: 124000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Linux is the primary target; users must not hunt for GStreamer plugins.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 AppImage bundles the pinned GStreamer runtime including nvcodec and va plugins and runs on Ubuntu LTS and Fedora
- [ ] #2 Flatpak manifest uses the freedesktop GStreamer extension and passes flatpak-builder in CI
- [ ] #3 Hardware encode works from both packages (verified)
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor: dependencies changed from the manual verify tasks (TASK-64/65) to the export pipeline, CLI render and pop-out. Hardware encode inside the packages is checked by the hardware workflow (TASK-116) on box and RunsOn after packaging, not before.
<!-- SECTION:NOTES:END -->
