---
id: TASK-94
title: 'MCP tool families: project, media, timeline, playback, export'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - mcp
milestone: m-6
dependencies:
  - TASK-93
  - TASK-63
references:
  - docs/PLAN.md
priority: high
ordinal: 115000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The core agent surface (§7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 project.* (new, open, save, list_sequences, settings), media.* (import, list, probe, relink, make_proxy), timeline.* (add/move/trim/split/delete clip, tracks, markers, get_state), playback.* (seek, play, pause, render_frame_png), export.* (list_presets, render, progress)
- [ ] #2 render_frame_png returns an image so an agent can look at the frame
- [ ] #3 Each tool has a description and JSON schema; an integration test calls every tool once
<!-- AC:END -->
