---
id: TASK-131
title: >-
  Windows CI regressed again: 31 min build, 19 min tests, runs cancelled at the
  timeout
status: In Progress
assignee:
  - '@opus-task-131'
created_date: '2026-09-11 00:06'
updated_date: '2026-09-11 08:33'
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
- [x] #1 Windows job on a warm no-change rerun completes in under 20 minutes, recorded with the run id in the notes
- [x] #2 The top five slowest Windows test binaries are listed in the notes with their durations, and any test moved or gated is justified in ci.yml comments
- [ ] #3 Windows timeout lowered back to 40 minutes and no run in the following three merges is cancelled at the limit
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure from the Actions logs rather than guessing: per-step and per-test-binary durations for the windows-latest job of runs 34574614657 / 34574256857 / 34564826872.
2. Root cause: the Windows Test step is 462 s of which only ~90 s is running tests. 353 s is cargo recompiling the workspace twice, because 'cargo build --workspace --all-targets', 'cargo test --workspace --exclude sub-ui', 'cargo test -p sub-ui' and 'cargo run -p subordinate' are four different package selections, so feature unification differs and each step invalidates the previous step's artifacts. The GUI smoke test pays another 55 s for the same reason.
3. Fix: one 'cargo test --workspace -- --test-threads=1' invocation, matching the Build step's selection exactly, so the test step compiles nothing. Serial execution also satisfies the lavapipe constraint that made sub-ui a separate invocation in the first place, so the split disappears on every OS.
4. Fix: the GUI smoke test runs the binary the Build step already produced instead of 'cargo run -p subordinate'.
5. Lower the Windows timeout from 90 back to 40 minutes and rewrite the ci.yml comments to record the measurement and the rationale.
6. Verify locally that 'cargo test --workspace --no-run' after 'cargo build --workspace --all-targets' compiles nothing, and record the slowest Windows test binaries in the notes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor measurement: warm no-change rerun of run 34564826872 gives windows-latest 19m26s (Build 7m, Test 7m, Clippy under 2m), inside the 20-minute criterion for that run. The cancelled-at-timeout pattern came from cold-cache runs; criterion 3 (timeout back to 40 and three merges without a cancellation) still needs doing, and the slowest Windows test binaries should be listed (criterion 2).

Diagnosis (from the Actions logs, not guessed). windows-latest job durations on the three most recent main runs: 34574614657 20m24s, 34574256857 19m38s, 34564826872 19m26s -- Build ~460 s, Test ~450 s, Clippy ~40 s, GUI smoke ~55 s in each. rust-cache restores cleanly on Windows (57 s restore step, hit) and the Defender exclusions do apply, so neither was the problem.

The Test step is 462 s of which only 89 s is tests actually executing. The rest is cargo recompiling the workspace, twice: 156 s before the first test ran and 197 s before the second. The cause is package selection. Cargo unifies features across the packages a command selects, so 'cargo build --workspace --all-targets', then 'cargo test --workspace --exclude sub-ui', then 'cargo test -p sub-ui', then 'cargo run -p subordinate -- --smoke-test' are four different feature resolutions in a row and each one invalidates the previous step's artifacts. Log evidence in run 34574614657: Build finished 07:40:18, the Test step then recompiled sub-plugin, sub-audio, sub-ui, subordinate-bench, subordinate-mcp, subordinate-cli and subordinate before the first test at 07:42:55, and recompiled ten workspace crates again between 07:43:51 and 07:47:08 for the sub-ui invocation. The GUI smoke step relinked four crates plus the binary for the same reason (07:48:49-07:49:44, 55 s).

Slowest five Windows test binaries in that run: sub-media seek_fixtures 9.30 s, sub-test-support sample_media_catalogue 5.35 s, sub-ui timeline_selection 3.84 s, sub-ui inspector 3.60 s, sub-ui timeline_transition 3.13 s. Everything else is under 3 s and the whole suite executes in 89 s, so the 19-minute Test step was never about slow tests and nothing needed moving or gating.

Change. .github/workflows/ci.yml: the Test step is now a single 'cargo test --workspace -- --test-threads=1', selecting exactly what the Build step selects; the two GUI smoke steps run target/debug/subordinate (subordinate.exe on Windows) instead of 'cargo run -p subordinate'; timeout-minutes is a flat 40 on every OS instead of 90 for Windows and 60 elsewhere. The comment block on the Test step records the measurement, the feature-unification rule and the five slowest binaries, and states that no test is skipped, gated or moved. docs/DEVELOPMENT.md, 'CI build speed', gains a bullet on keeping every cargo step's package selection identical and corrects the timeout paragraph.

Local verification in this worktree, with the CI environment variables (CARGO_INCREMENTAL=0, CARGO_PROFILE_DEV_DEBUG/TEST_DEBUG=line-tables-only):
- 'cargo build --workspace --all-targets' 13m21s cold, then 'cargo test --workspace --no-run' compiled only rayon, dify, egui_kittest and sub-ui in 48 s. Repeating test --no-run, then build --all-targets, then test --no-run again each finished in 0.2 s, so the two commands no longer invalidate each other -- the old pair recompiled the workspace both ways, which is the 353 s. The residual 48 s is a one-off delta, not a ping-pong.
- 'cargo clippy --workspace --all-targets -- -D warnings' clean, and target/debug/subordinate is byte-identical and still present afterwards, which is what the GUI smoke steps now depend on.
- 'cargo fmt --all --check' clean.
- The exact new test command, 'cargo test --workspace -- --test-threads=1', exits 0: 132 test binaries, 2010 passed, 14 ignored, 0 failed. Serial 23.0 s against 18.4 s parallel on this machine, so the lost parallelism is small.

Not verifiable here: this agent must not push, so no CI run exercises the change yet. Criterion 3's second half -- three merges with no cancellation -- needs the branch merged and is left unchecked.

Acceptance criteria. #1 checked: the windows-latest job of run 34564826872 took 19m26s and of run 34574256857 19m38s, both warm and both under 20 minutes, confirmed today against the Actions jobs API; run 34574614657 was 20m24s, which is why the change below matters even though the criterion was already met. #2 checked: the five slowest Windows binaries are listed above and in the ci.yml comment, and nothing was moved, skipped or gated -- the comment says so explicitly. #3 left unchecked: the timeout is lowered to 40 in this branch, but the three merges without a cancellation can only be observed after it lands on main.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The Windows job was never hanging and its tests were never slow: the whole suite executes in 89 seconds. It was cargo recompiling the workspace twice more after the Build step, because 'cargo build --workspace --all-targets', 'cargo test --workspace --exclude sub-ui', 'cargo test -p sub-ui' and 'cargo run -p subordinate' are four different package selections and cargo unifies features per selection, so each one invalidated the last -- 353 s of a 462 s Test step and 55 s of a 57 s smoke step in run 34574614657, with the same shape on Linux and macOS. ci.yml now runs one 'cargo test --workspace -- --test-threads=1' with the Build step's selection (the serial run is also what sub-ui needed on lavapipe, so its separate invocation disappears), invokes the already-built target/debug/subordinate for the GUI smoke tests, and drops the Windows timeout from 90 back to a flat 40; docs/DEVELOPMENT.md records the rule. Verified locally: build then test --no-run now compiles a 48 s delta once instead of the workspace twice and the two commands stay stable across repeats, clippy and fmt are clean, the binary survives the Clippy step, and the new test command exits 0 over 132 binaries and 2010 tests. Two of three criteria are proven; the third needs three merges on main to observe, so the task stays In Progress.
<!-- SECTION:FINAL_SUMMARY:END -->
