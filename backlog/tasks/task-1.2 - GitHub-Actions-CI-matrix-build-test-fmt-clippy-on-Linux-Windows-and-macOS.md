---
id: TASK-1.2
title: 'GitHub Actions CI matrix: build, test, fmt, clippy on Linux, Windows and macOS'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-08 21:16'
labels:
  - infra
milestone: m-0
dependencies:
  - TASK-1.1
references:
  - docs/PLAN.md
parent_task_id: TASK-1
priority: high
ordinal: 11000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Cross-platform is a core promise. Catching platform breakage per PR is far cheaper than at release, so the matrix must exist before real code arrives.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Workflow runs on ubuntu-latest, windows-latest and macos-latest for pushes and pull requests
- [ ] #2 Workflow runs cargo build, cargo test, cargo fmt --check and cargo clippy -D warnings on each OS
- [ ] #3 Cargo registry and target caches are used so a no-change run finishes in under 10 minutes
- [ ] #4 A failing test on any single OS fails the workflow
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. .github/workflows/ci.yml: on push/pull_request, matrix ubuntu-latest/windows-latest/macos-latest, fail-fast false
2. Steps: checkout, dtolnay/rust-toolchain@stable reading rust-toolchain.toml, Swatinem/rust-cache, cargo build, cargo test, cargo fmt --check, cargo clippy --all-targets -D warnings
3. GStreamer install step is TASK-1.3; leave a clearly marked placeholder
4. Validate YAML locally and run the same commands locally; note that an Actions run needs a GitHub remote
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Workflow written at .github/workflows/ci.yml: matrix ubuntu/windows/macos, fail-fast false, dtolnay/rust-toolchain pinned 1.93.1, Swatinem/rust-cache with per-OS shared key, steps build/test/fmt/clippy -D warnings. Validated with actionlint 1.7.12 (clean) and by running the same four commands locally (clean). Repo has no GitHub remote yet, so an actual Actions run has not been observed.
<!-- SECTION:NOTES:END -->
