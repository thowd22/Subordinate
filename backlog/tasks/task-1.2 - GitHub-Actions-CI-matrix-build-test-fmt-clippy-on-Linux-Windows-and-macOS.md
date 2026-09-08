---
id: TASK-1.2
title: 'GitHub Actions CI matrix: build, test, fmt, clippy on Linux, Windows and macOS'
status: Done
assignee:
  - '@claude'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-08 21:54'
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
- [x] #1 Workflow runs on ubuntu-latest, windows-latest and macos-latest for pushes and pull requests
- [x] #2 Workflow runs cargo build, cargo test, cargo fmt --check and cargo clippy -D warnings on each OS
- [x] #3 Cargo registry and target caches are used so a no-change run finishes in under 10 minutes
- [x] #4 A failing test on any single OS fails the workflow
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

Verified by Actions: run 34280612091 (first push) ran all three OS jobs and failed overall because only Windows (fmt) and macOS (test) failed while Linux passed, proving a single-OS failure fails the workflow. Run 34281067519 is green on all three with build/test/fmt/clippy steps. Linux took 15m due to a slow apt mirror; apt archive caching added, timing criterion pending the next run.

Cached no-change rerun https://github.com/thowd22/Subordinate/actions/runs/34282573104: ubuntu 137s, windows 117s, macos 133s (apt install 42s from the 285 MB archive cache). Criterion 3 met.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added .github/workflows/ci.yml running build, test, fmt and clippy -D warnings on ubuntu-26.04, windows-latest and macos-latest with Rust 1.93.1, Swatinem cache and apt archive cache. Verified by Actions runs: a single-OS failure failed the workflow (run 34280612091), all-green runs 34281067519 and 34282573104, and a cached no-change rerun completing in about two minutes per OS.
<!-- SECTION:FINAL_SUMMARY:END -->
