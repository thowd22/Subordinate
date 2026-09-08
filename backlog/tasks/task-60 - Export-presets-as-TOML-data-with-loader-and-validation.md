---
id: TASK-60
title: Export presets as TOML data with loader and validation
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - export
milestone: m-4
dependencies:
  - TASK-59
references:
  - docs/PLAN.md
priority: medium
ordinal: 81000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Presets are data and an extension point for plugins (§5.5).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Built-in presets: YouTube 1080p, YouTube 4K, ProRes-like mezzanine (software), H.265 archive, audio-only
- [ ] #2 Preset schema: container, video codec, bitrate or CRF, resolution, frame rate, audio codec and bitrate
- [ ] #3 Invalid presets fail with a SubError naming the field; user presets load from the config dir
<!-- AC:END -->
