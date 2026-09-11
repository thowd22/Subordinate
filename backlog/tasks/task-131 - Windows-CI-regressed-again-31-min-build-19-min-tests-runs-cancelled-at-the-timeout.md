---
id: TASK-131
title: >-
  Windows CI regressed again: 31 min build, 19 min tests, runs cancelled at the
  timeout
status: To Do
assignee: []
created_date: '2026-09-11 00:06'
updated_date: '2026-09-11 07:48'
labels:
  - infra
  - ci
milestone: m-0
dependencies:
  - TASK-126
references:
  - .github/workflows/ci.yml
priority: high
ordinal: 151000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
After TASK-125 the Windows job briefly ran in 8 minutes, but run 34539791304 shows Build 31m, Test 19m and Clippy 5m, cancelled at 60m, and the CI guard reports five of the last twelve runs on main cancelled at the limit; the guard raised the Windows timeout to 90 minutes, which is a stopgap not a fix. Every wave's merge-and-verify cycle now waits on Windows. Diagnose from the Actions logs rather than guessing: check whether rust-cache restores on Windows (key, size, hit), whether the 19-minute Test step is a few slow tests (kittest snapshots, GUI smoke, fixture generation) or the whole suite, and whether Defender exclusions actually applied. Consider splitting Windows into a build+clippy job and a test job, caching generated fixtures, skipping Linux-only steps, or moving heavy integration tests to Linux only with a documented rationale.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Windows job on a warm no-change rerun completes in under 20 minutes, recorded with the run id in the notes
- [ ] #2 The top five slowest Windows test binaries are listed in the notes with their durations, and any test moved or gated is justified in ci.yml comments
- [ ] #3 Windows timeout lowered back to 40 minutes and no run in the following three merges is cancelled at the limit
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor measurement: warm no-change rerun of run 34564826872 gives windows-latest 19m26s (Build 7m, Test 7m, Clippy under 2m), inside the 20-minute criterion for that run. The cancelled-at-timeout pattern came from cold-cache runs; criterion 3 (timeout back to 40 and three merges without a cancellation) still needs doing, and the slowest Windows test binaries should be listed (criterion 2).
<!-- SECTION:NOTES:END -->
