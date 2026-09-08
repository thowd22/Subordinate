---
id: TASK-48
title: 'Lock-free mixer graph: clip, track, master with gain, fades, mute and solo'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-47
  - TASK-12
references:
  - docs/PLAN.md
priority: high
ordinal: 69000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The real-time audio callback must never lock or allocate (§4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Mixer pulls from per-clip ring buffers, applies clip gain and fades, track gain, mute and solo, sums to master
- [ ] #2 Graph updates from the engine are applied via atomic swap of an immutable graph description
- [ ] #3 A test wraps the callback with an allocation-detecting allocator and asserts zero allocations over 10 seconds
<!-- AC:END -->
