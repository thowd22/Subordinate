---
id: TASK-9
title: >-
  Write WIT for the command world and run a hello-world component through
  wasmtime
status: To Do
assignee: []
created_date: '2026-09-08 20:53'
labels:
  - spike
  - plugins
milestone: m-6
dependencies:
  - TASK-5
references:
  - docs/PLAN.md
priority: high
ordinal: 9000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Plugins are WASM components with versioned WIT worlds following Zed's pattern (decision-6). The command world is the first and simplest: a plugin that calls back into the Command API host import. This proves the host-import shape before the other worlds are designed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 wit/subordinate-plugin.wit defines subordinate:plugin@0.1.0 with a command world and a command-api host interface
- [ ] #2 A Rust component built with cargo component adds a marker to a fixture project through the host import
- [ ] #3 wasmtime fuel or epoch limits terminate a deliberately looping plugin
<!-- AC:END -->
