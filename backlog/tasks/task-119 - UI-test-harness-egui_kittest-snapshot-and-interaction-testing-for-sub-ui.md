---
id: TASK-119
title: 'UI test harness: egui_kittest snapshot and interaction testing for sub-ui'
status: In Progress
assignee:
  - '@opus-task-119'
created_date: '2026-09-09 18:21'
updated_date: '2026-09-09 19:05'
labels:
  - ui
  - test
  - infra
milestone: m-2
dependencies:
  - TASK-19
  - TASK-28
references:
  - 'https://docs.rs/egui_kittest'
  - docs/PLAN.md
priority: high
ordinal: 139000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
UI regressions are currently only caught by hand. egui_kittest 0.36.2 (features eframe, snapshot, wgpu) renders real egui UI headlessly through wgpu on a software adapter, simulates input via AccessKit, and diffs PNG snapshots. It runs on the free GitHub-hosted runners with no desktop, so this is the zero-cost layer of UI testing; GPU runners are reserved for GPU-specific checks (TASK-118). Snapshot PNGs are committed to the repo, so keep them small (render at 800x600 or less, no LFS).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 sub-ui has a dev-dependency on egui_kittest with snapshot and wgpu features, and a test-support module that builds a Harness around any panel with a fixture project loaded from the committed sample project
- [ ] #2 Snapshot tests run in cargo test on all three CI OSes using the software adapter; mismatches upload the diff images as workflow artifacts
- [x] #3 Snapshot update procedure (UPDATE_SNAPSHOTS=1 cargo test -p sub-ui) and the tolerance settings are documented in docs/DEVELOPMENT.md
- [x] #4 Every committed snapshot PNG is under 150 KB and the whole snapshot directory under 5 MB
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add egui_kittest 0.36.2 (eframe, snapshot, wgpu) as a sub-ui dev-dependency.
2. Add crates/sub-ui/tests/support/mod.rs: a shared harness module that builds an egui_kittest Harness at a fixed 800x600 dark-theme size with the wgpu software renderer, loads the committed sample project (crates/sub-model/tests/fixtures/sample-project.sub) through sub_model::json, and skips cleanly when the machine enumerates no wgpu adapter.
3. Add kittest.toml at the workspace root pinning the snapshot output path and per-OS tolerances; commit one small demonstration snapshot (timeline panel over the fixture project) plus an AccessKit interaction test (history panel row click) that needs no GPU.
4. CI: upload crates/sub-ui/tests/snapshots diff/new PNGs as a workflow artifact when the test job fails, on all three OSes.
5. Document the harness, UPDATE_SNAPSHOTS=1 cargo test -p sub-ui, and the tolerance settings in docs/DEVELOPMENT.md; assert the snapshot size budget (<150 KB per PNG, <5 MB total) in a test.
6. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, cargo test -p sub-ui (GStreamer env sourced).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Harness landed as crates/sub-ui/tests/support/mod.rs (pulled in with 'mod support;'): a shared 800x600, one-pixel-per-point, dark-theme, wgpu HarnessBuilder; fixture_project()/fixture_sequence() load the committed crates/sub-model/tests/fixtures/sample-project.sub through sub_model::json; can_render() asks sub_render::RenderContext::headless() once per process so a machine with no ICD reports and passes instead of failing. Tolerances and the snapshot output path are in the new workspace-root kittest.toml (threshold 0.6; max_failed_pixels 10 Linux, 300 Windows/macOS); .new.png and .diff.png are gitignored.

crates/sub-ui/tests/ui_harness.rs proves the harness end to end: the fixture loads, a panel renders at exactly 800x600, the timeline panel over the fixture matches the committed snapshot harness_timeline_panel.png (26,887 bytes), an AccessKit click on a history-panel row yields HistoryAction::Undo { steps: 1 }, and the committed snapshot directory stays inside 150 KB per PNG / 5 MB total.

Verified on this machine (Linux, Mesa lavapipe): cargo test -p sub-ui all green (including the five ui_harness tests, snapshot recorded with UPDATE_SNAPSHOTS=1 and then re-compared), cargo fmt --all --check clean, cargo clippy --workspace --all-targets -- -D warnings clean.

AC #2 left unchecked: the CI half cannot be proven from here. The snapshot tests are part of the existing 'cargo test --workspace' step that already runs on ubuntu-26.04, windows-latest and macos-latest (lavapipe/WARP/Metal are already installed by that workflow), and a new 'Upload UI snapshot diffs' step (actions/upload-artifact@v4, if: failure(), if-no-files-found: ignore) publishes crates/sub-ui/tests/snapshots/**/*.new.png and *.diff.png as ui-snapshot-diffs-<os>. Neither the Windows/macOS run nor the artifact upload has been observed; the reference PNG was recorded on lavapipe, so the first CI run on the other two OSes may need the per-OS max_failed_pixels revisited. This task may not push, so that needs a CI run to confirm.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the egui_kittest UI test harness for sub-ui: egui_kittest 0.36.2 (eframe, snapshot, wgpu) as a dev-dependency, a shared tests/support module that builds a Harness around any panel at a fixed 800x600 dark frame over the committed sample project and skips cleanly with no wgpu adapter, workspace-root kittest.toml tolerances, a first committed snapshot of the timeline panel (26.9 KB), an AccessKit interaction test, a snapshot size-budget test, a CI step that uploads .new/.diff PNGs on failure, and a 'UI tests (egui_kittest)' section in docs/DEVELOPMENT.md covering UPDATE_SNAPSHOTS=1 cargo test -p sub-ui. Verified locally with cargo test -p sub-ui, cargo fmt --all --check and cargo clippy --workspace --all-targets -- -D warnings, all clean on Mesa lavapipe. AC #2 stays unchecked: running the snapshots on the Windows and macOS runners and the artifact upload can only be observed in CI, which this task cannot push.
<!-- SECTION:FINAL_SUMMARY:END -->
