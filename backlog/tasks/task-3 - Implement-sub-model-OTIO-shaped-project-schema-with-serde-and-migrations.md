---
id: TASK-3
title: 'Implement sub-model: OTIO-shaped project schema with serde and migrations'
status: To Do
assignee: []
created_date: '2026-09-08 20:53'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-2
references:
  - docs/PLAN.md
priority: high
ordinal: 3000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The native project format is JSON mirroring OTIO's Timeline/Stack/Track/Clip/Gap/Transition/Marker shape so a future OTIO export is trivial (decision: own format, not OTIO). Media items carry relative paths plus content hashes for relinking. Colour metadata is stored as tags only (decision-3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Project, Sequence, Track, Clip, Gap, Transition, Marker, MediaItem and Bin types serialize to and from JSON
- [ ] #2 Files carry a schema_version and a migration registry upgrades older versions
- [ ] #3 A sample project round-trips byte-identically through load and save
<!-- AC:END -->
