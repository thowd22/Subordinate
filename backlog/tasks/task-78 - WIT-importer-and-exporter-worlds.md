---
id: TASK-78
title: WIT importer and exporter worlds
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
references:
  - docs/PLAN.md
priority: medium
ordinal: 99000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Interchange and custom export targets live in plugins (§6.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 importer world: supported-extensions() and import(path) -> list<MediaOrSequenceSpec> applied by the host via commands
- [ ] #2 exporter world: presets() -> list<PresetDesc> and optional post-export(path) hook
- [ ] #3 Host wires importer results into the bin and exporter presets into the export panel
<!-- AC:END -->
