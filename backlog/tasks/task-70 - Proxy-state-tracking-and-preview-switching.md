---
id: TASK-70
title: Proxy state tracking and preview switching
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:07'
labels:
  - media
  - ui
milestone: m-5
dependencies:
  - TASK-69
  - TASK-59
references:
  - docs/PLAN.md
priority: high
ordinal: 91000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Preview uses proxies, export never does.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 MediaItem proxy state (none, generating, ready, stale) is shown in the bin
- [ ] #2 Viewer toggle uses proxies when ready; export pipeline always uses originals (test asserts)
- [ ] #3 Proxies are invalidated when the source content hash changes
- [ ] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the proxy indicator and preview switch: a committed snapshot of each proxy state and an interaction test for the switch
<!-- AC:END -->
