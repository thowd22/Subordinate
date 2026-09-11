---
id: TASK-134
title: Export honours an explicitly pinned encoder even when GStreamer ranks it NONE
status: To Do
assignee: []
created_date: '2026-09-11 14:48'
labels:
  - export
milestone: m-4
dependencies:
  - TASK-116
priority: medium
ordinal: 154000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
sub-export refuses any encoder element whose GStreamer rank is NONE. Every VA-API encoder (vah264enc, vah265enc) ships with rank NONE by design, so on AMD Linux an explicit --encoder vah264enc fails unless GST_PLUGIN_FEATURE_RANK is set, which is what the hardware workflow does as a workaround (TASK-116 notes). The automatic selection order may still skip rank-NONE elements, but a user or preset that names an encoder explicitly has already made the choice.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 subordinate-cli render --encoder vah264enc works on box without GST_PLUGIN_FEATURE_RANK, verified in the hardware workflow's AMD job
- [ ] #2 Automatic encoder selection behaviour is unchanged and covered by the existing capability-probe tests
- [ ] #3 The workaround environment variable is removed from hardware.yml
<!-- AC:END -->
