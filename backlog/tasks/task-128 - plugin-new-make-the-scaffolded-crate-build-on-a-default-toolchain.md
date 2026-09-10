---
id: TASK-128
title: 'plugin new: make the scaffolded crate build on a default toolchain'
status: To Do
assignee: []
created_date: '2026-09-10 21:58'
labels:
  - plugins
  - cli
milestone: m-6
dependencies:
  - TASK-102
priority: medium
ordinal: 148000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The TASK-102 runbook run hit this: the scaffolded crate declares rust-version = 1.95 (the SDK's floor) but writes no rust-toolchain.toml, so on a machine whose default toolchain is older, 'cargo build --release --target wasm32-wasip2' fails with a rust-version error the agent has to diagnose and work around (it found 'rustup run 1.95.0'). The scaffold's promise is that what it writes builds with no edits, so either it pins the toolchain it needs or its CLAUDE.md says in the build step which toolchain to use and how.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A crate written by plugin new builds with the plain command its CLAUDE.md prints, on a machine whose default toolchain is older than the SDK's rust-version
- [ ] #2 The scaffold test covering the build (SUBORDINATE_SCAFFOLD_BUILD=1) exercises that path
<!-- AC:END -->
