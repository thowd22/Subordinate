---
id: TASK-119
title: 'UI test harness: egui_kittest snapshot and interaction testing for sub-ui'
status: To Do
assignee: []
created_date: '2026-09-09 18:21'
labels:
  - ui
  - test
  - infra
milestone: m-2
dependencies:
  - TASK-19
  - TASK-28
references:
  - 'https://docs.rs/egui_kittest'
  - docs/PLAN.md
priority: high
ordinal: 139000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
UI regressions are currently only caught by hand. egui_kittest 0.36.2 (features eframe, snapshot, wgpu) renders real egui UI headlessly through wgpu on a software adapter, simulates input via AccessKit, and diffs PNG snapshots. It runs on the free GitHub-hosted runners with no desktop, so this is the zero-cost layer of UI testing; GPU runners are reserved for GPU-specific checks (TASK-118). Snapshot PNGs are committed to the repo, so keep them small (render at 800x600 or less, no LFS).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 sub-ui has a dev-dependency on egui_kittest with snapshot and wgpu features, and a test-support module that builds a Harness around any panel with a fixture project loaded from the committed sample project
- [ ] #2 Snapshot tests run in cargo test on all three CI OSes using the software adapter; mismatches upload the diff images as workflow artifacts
- [ ] #3 Snapshot update procedure (UPDATE_SNAPSHOTS=1 cargo test -p sub-ui) and the tolerance settings are documented in docs/DEVELOPMENT.md
- [ ] #4 Every committed snapshot PNG is under 150 KB and the whole snapshot directory under 5 MB
<!-- AC:END -->
