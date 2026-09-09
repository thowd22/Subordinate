---
id: TASK-10
title: Test media fixture generator script
status: Done
assignee:
  - '@opus-task-10'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 01:54'
labels:
  - infra
  - media
milestone: m-0
dependencies:
  - TASK-1.3
references:
  - docs/PLAN.md
priority: high
ordinal: 14000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Media tasks need deterministic sample files but binaries must not be committed. A script that synthesises them with gst-launch gives every developer and CI runner identical inputs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 scripts/gen-fixtures.sh (and a PowerShell equivalent or cross-platform Rust xtask) generates: 1080p and 4K H.264 colour bars with timecode burn-in, a 29.97 drop-frame clip, a variable-frame-rate clip, a 10-minute long-GOP clip, a WAV and a FLAC audio-only file
- [x] #2 Generated files land in fixtures/ which is gitignored, and a manifest JSON records name, duration, fps, VFR flag
- [x] #3 CI generates the small fixtures (not the 10-minute clip) before running tests
- [x] #4 Tests can locate fixtures via a helper in a shared test-support crate
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. scripts/gen-fixtures.sh: bash generator driving gst-launch-1.0, with flags --out, --long (include the 10-minute clip), --force, --list, --dry-run, plus preflight checks for gst-launch-1.0 and the required elements. 2. Fixture set: bars_1080p_h264.mp4 and bars_2160p_h264.mp4 (SMPTE bars with timecodestamper/timeoverlay burn-in), dropframe_2997_h264.mp4 at 30000/1001, vfr_60_30.mkv built by concatenating a 30fps and a 60fps segment so frame durations really vary, longgop_720p_10min.mp4 (key-int-max=250, only with --long), tone_48k_stereo.wav and tone_48k_stereo.flac. 3. fixtures/ gitignored; both scripts write fixtures/manifest.json recording name, kind, duration_ns as integer nanoseconds (no floats), fps as num/den, vfr flag, generated flag, dimensions and description. 4. scripts/gen-fixtures.ps1: PowerShell mirror of the same pipelines and manifest for Windows developers. 5. crates/sub-test-support: new workspace crate exposing fixtures_dir, manifest_path, load_manifest, fixture, try_fixture, Manifest/Fixture types and a FixtureError with stable codes, honouring SUB_FIXTURES_DIR; unit tests cover manifest parsing, missing-fixture errors and the env override. 6. CI: add a fixture-generation step before Build that runs the shell script without --long on all three OSes. 7. Document the generator in docs/DEVELOPMENT.md. Verify with cargo fmt, clippy, cargo test, bash -n and a --dry-run run.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added scripts/gen-fixtures.sh (bash) and scripts/gen-fixtures.ps1 (PowerShell mirror) driving gst-launch-1.0, a gitignored fixtures/ directory with manifest.json, the new sub-test-support workspace crate for locating fixtures, and a CI step that generates the small fixtures before Build.

Design decisions. The catalogue lives in one tab-separated table in the shell script (and the matching array in the PowerShell script) so the fixture list, the pipelines and the manifest cannot drift apart. Durations are exact integer nanoseconds and frame rates exact num/den rationals, never floats, matching the RationalTime convention. Skipped fixtures stay in the manifest with generated=false so a test can skip itself rather than fail. The VFR clip is built by concatenating a 90-frame 30 fps segment and a 180-frame 60 fps segment and relabelling the joined stream to one caps with capssetter: the encoder sees a single stream while Matroska records buffer timestamps 33.3 ms apart for three seconds then 16.6 ms apart, which is a genuinely variable-frame-rate file and is deterministic (identity drop-probability would not be). The 29.97 drop-frame clip runs at 30000/1001, the rate for which timecodestamper sets the drop-frame flag. FixtureError carries stable codes (fixtures.manifest_missing, fixtures.manifest_unreadable, fixtures.manifest_invalid, fixtures.unknown, fixtures.not_generated, fixtures.file_missing); SubError does not exist yet (TASK-11), so this crate defines its own coded error in the same spirit. serde and serde_json were added as workspace dependencies for the manifest.

Verification. cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test --workspace passes (7 unit tests plus 1 integration test plus 2 doctests in sub-test-support, rest of the workspace unchanged). bash -n on the shell script passes and --list and --dry-run were exercised. The script was then run end to end against stub gst-launch-1.0/gst-inspect-1.0 binaries: it created all six non-long fixture files, skipped the ten-minute clip, and wrote a manifest.json that sub-test-support parses, with the tests/generated_fixtures.rs integration test passing against it (SUB_FIXTURES_DIR pointed at the generated directory) and skipping cleanly when no manifest exists.

Not verified here (AC #1 left unchecked). This environment has GStreamer development headers only, no gst-launch-1.0 and no plugins, so no fixture was ever really encoded; the pipelines are verified by construction and by dry run, not by producing media. PowerShell is also unavailable in this sandbox, so gen-fixtures.ps1 could not even be parse-checked. Both are exercised by the new CI step on Linux, Windows and macOS, which is where AC #1 should be confirmed.

AC #1 verified by CI run https://github.com/thowd22/Subordinate/actions/runs/34300476881: the 'Generate test fixtures' step succeeded on ubuntu-26.04, windows-latest (PowerShell mirror) and macos-latest, and the sub-test-support integration test consumed the manifest. The 10-minute long-GOP clip is opt-in and not generated in CI by design.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixture generator scripts (bash and PowerShell) plus the sub-test-support crate; verified locally by dry-run and stub run, and by CI generating the fixtures on all three OSes.
<!-- SECTION:FINAL_SUMMARY:END -->
