---
id: TASK-130
title: >-
  plugin new: the command template answers JSON but the crate has no JSON
  dependency
status: To Do
assignee: []
created_date: '2026-09-10 21:59'
labels:
  - plugins
  - cli
milestone: m-6
dependencies:
  - TASK-102
priority: low
ordinal: 150000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the TASK-102 runbook run. A command plugin's run returns a JSON document, and the scaffolded src/lib.rs builds its answer with format! over hand-written braces because the generated Cargo.toml depends only on subordinate-sdk. The first thing the agent did after reading the template was add serde_json to Cargo.toml. Either the scaffold should ship that dependency, or the SDK should offer the answer type so a plugin never hand-rolls JSON.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A scaffolded command plugin can build its answer without the author adding a dependency
- [ ] #2 The CLAUDE.md the scaffold writes shows the way it intends
<!-- AC:END -->
