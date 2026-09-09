---
id: TASK-125
title: 'Cut Windows CI time: build 23 min and clippy 7 min versus 2 min on Linux'
status: To Do
assignee: []
created_date: '2026-09-09 22:25'
labels:
  - infra
  - ci
milestone: m-0
dependencies: []
references:
  - .github/workflows/ci.yml
  - 'https://github.com/Swatinem/rust-cache'
  - 'https://github.com/Mozilla-Actions/sccache-action'
priority: high
ordinal: 145000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI run 34407960264 on main took 38 minutes on windows-latest (Build 23m, Clippy 7m, cache save 2m) while ubuntu-26.04 took 5m and macos-latest 7m. Every wave of agent merges waits on Windows, and the CI guard already raised the job timeout to 60 minutes as a stopgap. Suspects: dev profile in Cargo.toml compiles all dependencies at opt-level 3 (slow on cache misses), Swatinem cache misses because Cargo.lock changes on nearly every merge, full debuginfo inflating MSVC link times, and Windows Defender scanning the target directory on hosted runners. Decide based on measured step timings, not guesses.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Windows job on a no-change rerun of main completes in under 15 minutes, measured from the Actions timing and recorded in the task notes
- [ ] #2 Cache hit rate for the Windows rust-cache restore is reported in the job log and is non-zero on the second run
- [ ] #3 Whatever profile or environment changes are made keep local developer builds unchanged (no change to default cargo build behaviour outside CI) and are explained in docs/DEVELOPMENT.md
- [ ] #4 Job timeout is lowered back to 40 minutes
<!-- AC:END -->
