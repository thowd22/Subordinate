---
id: TASK-75
title: 'WIT host interface: command-api, logging and project queries'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-9
  - TASK-5.4
references:
  - docs/PLAN.md
priority: high
ordinal: 96000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every plugin world imports the same host interface; it is the plugin-side view of the Command API (decision-6, decision-7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 wit/ defines subordinate:plugin@0.1.0 with a host interface exposing run-command(json) -> result, query(json) -> json, log(level, msg) and project metadata accessors
- [ ] #2 Types use WIT records for RationalTime and IDs rather than opaque strings where practical
- [ ] #3 wit-bindgen generates host bindings in sub-plugin and guest bindings in the SDK crate without warnings
<!-- AC:END -->
