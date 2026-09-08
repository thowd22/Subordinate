---
id: TASK-28
title: 'Timeline panel: ruler, track lanes, clip rectangles, scroll and zoom'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-27
references:
  - docs/PLAN.md
priority: high
ordinal: 49000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The central editing surface, painted directly with egui's Painter for performance.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Ruler shows timecode ticks appropriate to zoom; tracks render top-down with headers
- [ ] #2 Clips render with name, colour by media type and trimmed edge indicators
- [ ] #3 Ctrl+wheel zooms around cursor, wheel scrolls; 500 clips render at 60 fps (measured)
<!-- AC:END -->
