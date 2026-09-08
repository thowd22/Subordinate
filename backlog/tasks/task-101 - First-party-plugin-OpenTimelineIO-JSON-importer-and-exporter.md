---
id: TASK-101
title: 'First-party plugin: OpenTimelineIO JSON importer and exporter'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
  - first-party
milestone: m-6
dependencies:
  - TASK-78
  - TASK-91
references:
  - docs/PLAN.md
priority: medium
ordinal: 122000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Interchange lives in plugins; OTIO is the sensible target (§3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Exports a sequence as OTIO JSON with tracks, clips, gaps, transitions and markers
- [ ] #2 Imports OTIO JSON produced by the exporter (round-trip test) and by Kdenlive's native OTIO export
- [ ] #3 Effects are documented as not exported
<!-- AC:END -->
