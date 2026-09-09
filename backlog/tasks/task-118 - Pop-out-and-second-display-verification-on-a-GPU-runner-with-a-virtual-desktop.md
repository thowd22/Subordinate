---
id: TASK-118
title: Pop-out and second-display verification on a GPU runner with a virtual desktop
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
labels:
  - infra
  - gpu
  - ui
milestone: m-8
dependencies:
  - TASK-116
priority: medium
ordinal: 138000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The decode spike and the pop-out viewer have criteria that need a desktop session with two displays. A Linux GPU runner can run a virtual desktop (Xvfb or a headless Wayland compositor with two outputs) so the pop-out viewport can be exercised by a scripted test; RDP on the Windows runner supports multiple monitors for manual checks.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A hardware.yml job starts a two-output virtual display on the NVIDIA Linux runner and runs the app's pop-out smoke test, capturing a screenshot of each output as an artifact
- [ ] #2 Procedure for an interactive RDP session with two monitors on the Windows GPU runner is documented in docs/DEVELOPMENT.md
<!-- AC:END -->
