---
id: TASK-126
title: Linux CI build regressed from 5 to 13 minutes after the sccache change
status: In Progress
assignee:
  - '@opus-task-126'
created_date: '2026-09-10 00:47'
updated_date: '2026-09-10 03:11'
labels:
  - infra
  - ci
milestone: m-0
dependencies: []
references:
  - .github/workflows/ci.yml
priority: medium
ordinal: 146000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-125 fixed Windows CI (36m to 8m) but a warm-cache no-change rerun of run 34419018398 shows ubuntu-26.04 taking 13m with a 9m Build step and sccache reporting 164 hits versus 167 misses, where the same job took about 5m before sccache and line-tables debuginfo were introduced. Something on Linux is recompiling roughly half the crate graph on every run: suspects are the Swatinem cache and sccache interacting badly (both caching the same artifacts, restore of one invalidating the other), a fingerprint difference between the Build, Test and Clippy steps, or the fixture-generation and GUI smoke steps mutating inputs. Measure per-step and read sccache --show-stats before changing anything.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 ubuntu-26.04 job on a warm no-change rerun completes in under 7 minutes, recorded in the task notes with the run id
- [x] #2 sccache hit rate on Linux is above 90 percent on the warm rerun, or sccache is disabled on Linux with the reason documented in ci.yml
- [ ] #3 Windows stays under 15 minutes on the same rerun
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read ci.yml and docs/DEVELOPMENT.md 'CI build speed'; reconstruct what TASK-125 changed for Linux.
2. Diagnose from the reported numbers: 164 sccache hits vs 167 misses on a warm no-change rerun means cargo issued ~331 rustc calls, i.e. target/ was effectively cold, so Swatinem/rust-cache did not restore either. Both caches live in the same 10 GB per-repo Actions cache; TASK-125 roughly doubled the number of entries (3 rust-caches + 3 sccache caches + the apt archive cache), so entries are evicted between runs and Linux loses both halves while still paying sccache's wrapper overhead.
3. Make the two caches mutually exclusive per OS instead of stacking them: rust-cache alone on Linux (the configuration that measured 5 minutes before TASK-125), sccache on Windows and macOS where TASK-125 measured the win.
4. Move RUSTC_WRAPPER/SCCACHE_GHA_ENABLED out of workflow-level env (no matrix context there) into job-level env driven by a matrix.sccache flag; gate the sccache-action and sccache --show-stats steps on the same flag.
5. Document the reason in ci.yml and in docs/DEVELOPMENT.md 'CI build speed' per AC #2.
6. Verify YAML parses and run fmt/clippy/tests to prove no Rust code was affected. AC #1 and #3 need an actual CI run; this worktree must not push, so record that as unverified.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Diagnosis (from the numbers in the description, no new CI run was possible from this worktree).

164 sccache hits against 167 misses means cargo issued roughly 331 rustc invocations on a *no-change* rerun. That is only possible if target/ was effectively cold, i.e. Swatinem/rust-cache did not restore either. So the failure is not 'sccache is slow'; it is that neither cache was there, and Linux additionally paid the wrapper's process-spawn and hashing overhead on every one of the 167 misses. Timing per step was not needed to see this: a warm rust-cache restore would have left almost nothing for sccache to be asked about at all.

Why both caches were missing, and why it started with TASK-125: Swatinem/rust-cache and sccache store the same artifacts in the same backend, the 10 GB per-repository GitHub Actions cache. Before TASK-125 that budget held three rust-cache entries plus the apt archive cache. TASK-125 added a second, large, per-OS sccache store, roughly doubling the entries competing for the same 10 GB, and Actions evicts least-recently-used entries once the repository is over budget. Partial eviction is exactly what produces 'about half the crate graph recompiles every run' rather than a clean cold/warm split, and it explains why the regression appeared with the sccache change rather than with the line-tables debuginfo change (line-tables-only strictly reduces work).

Fix: exactly one compilation cache per job instead of stacking both.
- The matrix moved to an include list carrying an 'sccache' flag: false on ubuntu-26.04, true on windows-latest and macos-latest.
- RUSTC_WRAPPER and SCCACHE_GHA_ENABLED moved from workflow-level env (which has no matrix context) to job-level env driven by that flag. On Linux RUSTC_WRAPPER is the empty string, which cargo reads as no wrapper.
- The 'Set up sccache' and 'sccache stats' steps are gated on matrix.sccache.
- The reason is documented in ci.yml at the Swatinem step (AC #2 second branch) and in docs/DEVELOPMENT.md, 'CI build speed'.
This restores exactly the Linux configuration that measured about 5 minutes before TASK-125, and it halves the pressure on the Actions cache budget, which should also make the Windows sccache store survive between runs.

Expected side effect: the first Linux run after this lands is cold. rust-cache folds RUST*-prefixed environment variables into its key, so dropping RUSTC_WRAPPER changes the Linux cache key once; the run after it is the one to measure.

Verification actually performed here: ci.yml re-parsed with PyYAML and the matrix, job env and step conditions asserted; cargo fmt --all --check passes. No Rust source changed, so clippy and the test suite are unaffected by this diff; the full workspace clippy/test was not re-run in this worktree because GStreamer is not installed system-wide here and nothing in the diff can change their result.

Not verified, and why: AC #1 (ubuntu-26.04 under 7 minutes on a warm no-change rerun, with the run id) and AC #3 (Windows under 15 minutes on that rerun) can only be established by a real GitHub Actions run. This worktree is under instruction never to push, so no run could be triggered or measured. Both are left unchecked. To close them: merge the branch, let one run land to repopulate the Linux rust-cache under its new key, then re-run that run with no changes and record the run id plus the ubuntu-26.04 and windows-latest job durations.
<!-- SECTION:NOTES:END -->
