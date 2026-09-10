---
id: TASK-98
title: 'Plugins panel: installed list, status, errors, enable/disable'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:07'
labels:
  - ui
  - plugins
milestone: m-6
dependencies:
  - TASK-85
  - TASK-43
references:
  - docs/PLAN.md
priority: medium
ordinal: 119000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Non-agent users need to manage plugins too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Panel lists plugins with version, worlds, capabilities and load status
- [ ] #2 Enable, disable, remove and open-folder actions
- [ ] #3 Reload errors display inline
- [ ] #4 Hot-reload errors from --dev installs display inline in the panel (moved from TASK-86)
- [ ] #5 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the plugins panel: committed snapshots of the installed list in its healthy and error states, and an interaction test for enable/disable
<!-- AC:END -->
