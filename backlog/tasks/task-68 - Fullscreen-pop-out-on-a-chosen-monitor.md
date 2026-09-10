---
id: TASK-68
title: Fullscreen pop-out on a chosen monitor
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:07'
labels:
  - ui
milestone: m-5
dependencies:
  - TASK-67
references:
  - docs/PLAN.md
priority: medium
ordinal: 89000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Client-facing review on a TV or second monitor.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Monitor picker lists displays; fullscreen shows only the frame on black
- [ ] #2 Esc exits; the choice persists in settings
- [ ] #3 Works on Wayland, X11, Windows and macOS (verified on each)
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers whatever fullscreen exposes in a panel (the monitor picker, the toggle) as an interaction test; the real fullscreen presentation stays with TASK-118
<!-- AC:END -->
