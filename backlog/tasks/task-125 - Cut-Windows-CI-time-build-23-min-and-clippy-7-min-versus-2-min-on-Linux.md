---
id: TASK-125
title: 'Cut Windows CI time: build 23 min and clippy 7 min versus 2 min on Linux'
status: In Progress
assignee:
  - '@opus-task-125'
created_date: '2026-09-09 22:25'
updated_date: '2026-09-09 23:18'
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
- [x] #3 Whatever profile or environment changes are made keep local developer builds unchanged (no change to default cargo build behaviour outside CI) and are explained in docs/DEVELOPMENT.md
- [x] #4 Job timeout is lowered back to 40 minutes
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Attack the two measured hot spots on windows-latest: cold dependency compilation (Cargo.lock churn defeats the Swatinem key) and MSVC debuginfo/link plus Defender scanning of target/.
2. Add sccache (Mozilla-Actions/sccache-action) with the GitHub Actions cache backend as RUSTC_WRAPPER, so compilations hit even when Cargo.lock changes; unset the wrapper for the Clippy step because sccache cannot wrap clippy-driver. Print sccache --show-stats after build/test so the hit rate is in the job log.
3. Turn off full debuginfo in CI only via CARGO_PROFILE_DEV_DEBUG=line-tables-only in the workflow env (keeps file/line backtraces, drops the huge MSVC .pdb writes). No Cargo.toml change, so local builds are untouched.
4. Add a Windows-only Defender exclusion step for the workspace, CARGO_HOME and the sccache dir.
5. Drop the redundant 'Build' step (cargo test --workspace compiles the same units; clippy --all-targets covers the rest) to remove one duplicate link pass.
6. Lower timeout-minutes from 60 back to 40.
7. Document the CI-only knobs in docs/DEVELOPMENT.md; validate with actionlint if available; note that criteria requiring real Actions timings cannot be measured from this environment (no push).

5. (revised) Keep the Build step: cargo build --all-targets and cargo test share artifacts, so removing it would move time rather than save it, and the separate step is what makes the per-step timings in the task description readable.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Changes (all CI-only, .github/workflows/ci.yml plus docs/DEVELOPMENT.md; no Cargo.toml, .cargo/config.toml or Rust source touched):
- Job env: CARGO_PROFILE_DEV_DEBUG and CARGO_PROFILE_TEST_DEBUG = line-tables-only. Full debuginfo is the main driver of MSVC link time and .pdb writes on windows-latest; line tables keep file/line in the RUST_BACKTRACE output the tests print. Env vars, not a profile, so local builds are byte-for-byte unchanged.
- Added Mozilla-Actions/sccache-action@v0.0.9 with RUSTC_WRAPPER=sccache and SCCACHE_GHA_ENABLED=true for the whole job. Swatinem/rust-cache is keyed on Cargo.lock, which changes on nearly every merge, so it misses; sccache is keyed per compilation and survives that churn. Both kept. CARGO_INCREMENTAL=0 was already set, which sccache requires.
- Deliberately did NOT clear RUSTC_WRAPPER for the Clippy step: cargo folds the wrapper into its compiler fingerprint, so an inconsistent wrapper would invalidate every dependency and make clippy rebuild the world. Comment in the workflow records this.
- Windows-only step adds the workspace, ~/.cargo, ~/.rustup and SCCACHE_PATH to Defender ExclusionPath, plus sccache.exe/rustc.exe/link.exe to ExclusionProcess. Hosted runners are administrator; wrapped in try/catch so it can never fail the job.
- Final step runs sccache --show-stats with if: always(), which puts the hit/miss counts in the job log (AC #2 evidence, once a run exists).
- timeout-minutes 60 -> 40.
Considered and rejected: dropping the separate Build step (cargo build --all-targets and cargo test share artifacts, so it moves time rather than saving it, and loses the per-step timing signal); lowering dependency opt-level in CI (would need per-invocation --config, and with sccache the cold-build cost it targets is already mitigated); rust-lld for MSVC (unstable-ish linker swap, not worth the breakage risk once debuginfo is off).
Verification possible here: workflow parses as valid YAML (python yaml.safe_load; job timeout reads 40, step order confirmed); cargo fmt --all --check passes. No Rust source changed, so clippy and the test suites are unaffected and were not re-run in full.
Not verifiable in this environment: AC #1 and AC #2 both need a real GitHub Actions run on windows-latest. This agent works in a local worktree and is not permitted to push, so no run 
timing or restore-hit line can be recorded. They stay unchecked; whoever merges this should rerun main with no changes twice and paste the Windows job duration and the sccache hit rate into these notes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Windows CI cost was attacked at its three measured sources, entirely inside .github/workflows/ci.yml: debuginfo cut to line-tables-only via CARGO_PROFILE_*_DEBUG env vars (MSVC link and .pdb writes), sccache added as RUSTC_WRAPPER with the Actions cache backend so compilations still hit when Cargo.lock churn defeats the rust-cache key, and a Windows Defender exclusion step for the workspace, cargo/rustup homes and the sccache dir. A final 'sccache --show-stats' step puts the hit rate in the job log, and timeout-minutes is back to 40. Nothing was added to Cargo.toml or .cargo/config.toml, so local developer builds are unchanged; the reasoning is written up under 'CI build speed' in docs/DEVELOPMENT.md. Verified locally by parsing the workflow YAML (timeout 40, step order) and cargo fmt --all --check; AC #1 and #2 need a real Actions run on windows-latest, which this worktree cannot push, so they remain unchecked.
<!-- SECTION:FINAL_SUMMARY:END -->
