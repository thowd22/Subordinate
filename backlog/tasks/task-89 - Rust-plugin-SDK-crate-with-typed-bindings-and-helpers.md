---
id: TASK-89
title: Rust plugin SDK crate with typed bindings and helpers
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
  - sdk
milestone: m-6
dependencies:
  - TASK-80
  - TASK-76
  - TASK-81
references:
  - docs/PLAN.md
priority: high
ordinal: 110000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Agents write most plugins in Rust; the SDK hides WIT boilerplate.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 subordinate-sdk crate wraps guest bindings with ergonomic types, a run_command helper and serde-based param structs
- [ ] #2 Docs.rs-style documentation with an example per world
- [ ] #3 Published under MIT OR Apache-2.0 (decision-2)
<!-- AC:END -->
