---
id: TASK-1.1
title: 'Scaffold Cargo workspace, toolchain pin, fmt and clippy config'
status: Done
assignee:
  - '@claude'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-08 21:15'
labels:
  - infra
milestone: m-0
dependencies: []
references:
  - docs/PLAN.md
parent_task_id: TASK-1
priority: high
ordinal: 10000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
First code in the repo. The layered crate layout from docs/PLAN.md §4 must exist before any other task can land, and lint settings must be fixed early so later PRs are not dominated by style churn.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Cargo.toml workspace lists crates sub-time, sub-model, sub-edit, sub-media, sub-audio, sub-render, sub-export, sub-command, sub-plugin, sub-ui and bins subordinate, subordinate-cli, subordinate-mcp
- [x] #2 rust-toolchain.toml pins a stable channel and rustfmt.toml plus clippy lints in workspace Cargo.toml are committed
- [x] #3 cargo build, cargo test, cargo fmt --check and cargo clippy -D warnings all pass on Linux with the empty crates
- [x] #4 Each crate has a lib.rs or main.rs with a one-paragraph doc comment stating its responsibility from the plan
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Root Cargo.toml workspace (resolver 3) listing crates/* and bins/* with shared [workspace.package] and [workspace.lints]
2. rust-toolchain.toml pinning stable 1.93; rustfmt.toml
3. Create each crate with lib.rs doc comment; bins with main.rs stubs
4. Run cargo build, test, fmt --check, clippy -D warnings
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Workspace: 10 crates + 3 bins, resolver 3, edition 2024, workspace lints (clippy all+pedantic warn, missing_docs, unsafe_code warn). Added clippy.toml doc-valid-idents for product names (GStreamer, OpenTimelineIO, ...) so pedantic doc_markdown accepts prose. Validation: cargo build, cargo test (13 crates, 10 smoke tests), cargo fmt --check, cargo clippy --all-targets -D warnings all pass on Linux with rustc 1.93.1.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Scaffolded the Cargo workspace with all crates and bins from PLAN.md §4, pinned toolchain 1.93.1, rustfmt and clippy config. Verified by running build, test, fmt --check and clippy -D warnings locally, all clean.
<!-- SECTION:FINAL_SUMMARY:END -->
