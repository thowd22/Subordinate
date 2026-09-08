---
id: TASK-3.1
title: Stable identifiers and core entity types for the project model
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-2.1
  - TASK-11
references:
  - docs/PLAN.md
parent_task_id: TASK-3
priority: high
ordinal: 19000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every entity needs an ID that survives save/load and is safe to reference from plugins and MCP clients (§5.6).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Typed newtype IDs (ProjectId, SequenceId, TrackId, ClipId, MediaId, BinId, MarkerId) backed by UUIDv7 with serde as strings
- [ ] #2 Structs for Project, Sequence, Track (kind Video or Audio), Clip, Gap, Transition (Crossfade), Marker, MediaItem, Bin exist with doc comments mapping each to its OTIO counterpart
- [ ] #3 Sequence settings hold resolution, frame rate, sample rate and colour tags (space, transfer, primaries) per decision-3
<!-- AC:END -->
