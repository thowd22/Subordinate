---
id: TASK-57
title: Encoder capability probe and selection order
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - export
milestone: m-4
dependencies:
  - TASK-24
references:
  - docs/PLAN.md
priority: high
ordinal: 78000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Encoder availability differs per machine (§5.5). Export must pick the best available element and let users override.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Probe enumerates nvh264enc/nvh265enc/nvav1enc, vah264enc/vah265enc, amfh264enc/amfh265enc, vtenc_h264/vtenc_h265, mfh264enc, x264enc/x265enc and tests each can reach READY state
- [ ] #2 Selection follows the plan's order per codec with a user override in settings
- [ ] #3 Result is cached per session and shown in the diagnostics panel
<!-- AC:END -->
