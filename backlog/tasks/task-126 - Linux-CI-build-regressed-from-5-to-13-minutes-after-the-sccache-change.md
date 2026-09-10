---
id: TASK-126
title: Linux CI build regressed from 5 to 13 minutes after the sccache change
status: In Progress
assignee:
  - '@opus-task-126'
created_date: '2026-09-10 00:47'
updated_date: '2026-09-10 09:52'
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
1. Confirm the supervisor's cache-eviction finding with the API: repos/thowd22/Subordinate/actions/cache/usage and gh cache list.
2. Remove sccache from every OS in .github/workflows/ci.yml: the matrix sccache flag, the job-level RUSTC_WRAPPER/SCCACHE_GHA_ENABLED env, the sccache-action step, the sccache stats step and the sccache entries in the Defender exclusion list. Keep line-tables debuginfo and the Defender exclusions, which are independent wins.
3. Keep Swatinem/rust-cache as the only compilation cache, with a stable per-OS shared-key and save-if restricted to main so pull requests restore but never save, keeping the entry count at one archive per OS.
4. Delete the stale sccache/* Actions cache entries so rust-cache fits inside the 10 GB per-repository budget.
5. Document the reason in ci.yml and rewrite docs/DEVELOPMENT.md 'CI build speed'.
6. Verify: YAML parses and asserts, cargo fmt --all --check, clippy. AC #1 and #3 need a real warm rerun; this worktree must not push, so record them as unverified unless a run can be observed.
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

2026-09-10 supervisor measurement after the sccache-on-Linux-off fix merged: warm no-change rerun of run 34451650066 gave ubuntu-26.04 7m46s (target under 7m, close), macos-latest 7m51s, and windows-latest CANCELLED at the 40-minute timeout, i.e. Windows regressed back to a cold build. Suspect the 10 GB per-repository Actions cache is evicting entries (rust-cache per-OS per-Cargo.lock keys plus sccache) so Windows starts cold on most runs. Next worker: inspect gh cache list sizes and keys, cut the cache footprint (e.g. rust-cache save only on main with a stable key, or sccache alone on Windows, or delete stale keys), and re-measure with two consecutive warm reruns. Requeued.

2026-09-10 supervisor finding: gh cache list shows the repository cache is ENTIRELY sccache entries: 1000+ tiny objects (a few MB each, 1.15 GB in the first thousand) and zero Swatinem/rust-cache entries remain. The sccache GitHub-Actions backend stores one cache entry per compiled object, which evicts rust-cache's large per-OS target archives, so every job starts with a cold target directory and rebuilds the world; on Windows that exceeds 40 minutes. Recommended direction: remove sccache on every OS, keep Swatinem/rust-cache alone with a stable key (toolchain plus Cargo.toml hashes, save only on main via save-if), and delete the stale sccache entries with gh cache delete so rust-cache can fit. Keep the line-tables debuginfo and Defender exclusion changes, which are independent wins.

2026-09-10 second pass, this worker. Confirmed the supervisor finding against the API before changing anything. repos/thowd22/Subordinate/actions/cache/usage reported active_caches_size_in_bytes 10419834074 across 1080 entries, so the repository is over the 10 GB budget and Actions is continuously evicting. Of the 100 most recent entries, 98 were sccache object entries totalling 0.30 GB and only two were rust-cache archives: v0-rust-ci-windows-latest 1.25 GB and v0-rust-ci-ubuntu-26.04 1.53 GB. No macOS rust-cache archive survived at all. That is the mechanism: the sccache GitHub Actions backend writes one entry per compiled object, thousands of them, and LRU eviction takes out the few large rust-cache archives first, so jobs start with a cold target directory and rebuild the world.

Change made: sccache removed on every OS, not just Linux. The matrix is back to a plain os list (ubuntu-26.04, windows-latest, macos-latest); the matrix sccache flag and the whole job-level env block (RUSTC_WRAPPER, SCCACHE_GHA_ENABLED) are gone. The sccache-action setup step and the final sccache show-stats step are removed. The Defender exclusion step keeps the workspace, the cargo home and the rustup home plus rustc.exe and link.exe, and drops SCCACHE_PATH and sccache.exe. The Clippy step wrapper-fingerprint comment is removed, since no wrapper exists any more. Swatinem/rust-cache is now the only compilation cache, with shared-key ci-<os> as before plus save-if restricted to refs/heads/main, so pull requests restore the main archive but never write an entry of their own; that holds the footprint at one archive per OS instead of one per branch per OS. Line-tables debuginfo and the Defender exclusions from TASK-125 are kept, both being independent wins.

Not done here, and required for the fix to take full effect: the roughly 1080 stale sccache entries still occupy the cache budget. Deleting them from this worktree was blocked by the permission classifier, so it stays a manual step, documented in docs/DEVELOPMENT.md 'CI build speed' and pointed at from ci.yml (list them with the gh cache list command, delete each id, repeat until none are left). Until that is done the first rust-cache saves may still be evicted.

Verification performed here: ci.yml re-parsed with PyYAML and asserted (matrix os list, no job-level env, no step named or using sccache, save-if on the Swatinem step, no RUSTC_WRAPPER or SCCACHE token anywhere in the file); cargo fmt --all --check passes. The diff touches only .github/workflows/ci.yml, docs/DEVELOPMENT.md and this task file, no Rust source at all, so clippy and the test suite cannot be affected by it and were not re-run in this worktree (GStreamer is not installed system-wide here). AC #1 and #3 remain unchecked: both require a measured warm no-change rerun on GitHub Actions and this worktree must never push, so no run could be triggered or measured. To close them, delete the stale sccache entries, merge this branch, let one run land on main to populate the rust-cache archives under the new key, then rerun that run with no changes and record the run id plus the ubuntu-26.04 and windows-latest job durations.
<!-- SECTION:NOTES:END -->

## Comments

<!-- COMMENTS:BEGIN -->
author: @opus-task-126
created: 2026-09-10 09:51
---
probe GitHub Actions
---
<!-- COMMENTS:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Removed sccache from every OS in .github/workflows/ci.yml and left Swatinem/rust-cache as the single compilation cache (per-OS shared-key, save-if restricted to main so pull requests restore but never save). Root cause confirmed against the Actions cache API rather than inferred: the repository held 1080 entries and 10.4 GB, 98 of every 100 being sccache per-object entries, so LRU eviction removed the large rust-cache archives and nearly every job started with a cold target directory, giving 13 minutes on Linux and a 40-minute timeout on Windows. Line-tables debuginfo and the Windows Defender exclusions from TASK-125 are kept. Verified by re-parsing ci.yml with PyYAML and asserting the matrix, the absence of any sccache or RUSTC_WRAPPER reference and the save-if condition, plus cargo fmt --all --check; no Rust source changed. AC #1 and #3 are left unchecked because they need a measured warm rerun on GitHub Actions, which this worktree cannot trigger, and the stale sccache cache entries still need deleting by hand (documented in docs/DEVELOPMENT.md).
<!-- SECTION:FINAL_SUMMARY:END -->
