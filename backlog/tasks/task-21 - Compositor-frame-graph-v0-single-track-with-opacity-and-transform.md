---
id: TASK-21
title: 'Compositor frame graph v0: single track with opacity and transform'
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - render
milestone: m-1
dependencies:
  - TASK-20
  - TASK-12
references:
  - docs/PLAN.md
priority: high
ordinal: 42000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Establishes the render-graph structure that later multi-track and effects work extends (§5.3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 render(sequence, time) resolves the clip under the playhead, samples its frame and draws it with opacity and position/scale/rotation into a target texture
- [ ] #2 Output resolution follows sequence settings; letterboxing for mismatched aspect is correct
- [ ] #3 Renders to an offscreen texture usable both by egui and by readback
<!-- AC:END -->
