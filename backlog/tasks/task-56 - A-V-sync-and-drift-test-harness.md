---
id: TASK-56
title: A/V sync and drift test harness
status: Done
assignee:
  - '@opus-task-56'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 13:43'
labels:
  - test
  - audio
milestone: m-3
dependencies:
  - TASK-50
references:
  - docs/PLAN.md
priority: medium
ordinal: 77000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Phase 3 exit criterion must be measurable.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Harness plays the long fixture headlessly, sampling audio position against the displayed frame's PTS every second
- [x] #2 Reports maximum drift in frames; CI asserts under one frame on Linux
- [x] #3 Results appended to docs/PERFORMANCE.md
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add an A/V sync mode to bins/subordinate-bench (new module sync.rs, selected with --sync): it opens the long fixture, runs the real audio path (mixer -> OutputRenderer at 48 kHz into a 44.1 kHz device so the resampler is exercised) to drive the AudioClock, follows that master with sub_edit::playback::PlaybackScheduler, decode-forwards sub_media::Decoder to the frame the scheduler picked, and samples the audible position against the displayed frame's PTS once per second of timeline.
2. Drift is exact integer arithmetic in milli-frames: offset = audible position minus the displayed frame's PTS, expressed in frames; drift is the signed distance outside that frame's own display interval, so an in-sync picture reads zero. No floats anywhere.
3. Report: a sync section in the JSON report (report.rs) plus summary lines -- fixture, decoder, seconds played, samples, frames shown, dropped frames, max drift in milli-frames and where it happened. The harness exits non-zero with a stable code (bench.drift_exceeded) when max drift reaches one frame, which is how CI asserts; an absent fixture is a skipped section and exit 0.
4. CI: a Linux-only step that generates the long fixture (gen-fixtures.sh --long) and runs subordinate-bench --sync, uploading the report.
5. Tests: unit tests for the drift arithmetic, the sample cadence and argument parsing; the full run is exercised in CI where the long fixture exists.
6. Append an A/V sync and drift section to docs/PERFORMANCE.md describing what is measured, how to run it and how to read it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented as a second mode of the existing benchmark binary rather than a new crate: 'subordinate-bench --sync' (bins/subordinate-bench/src/sync.rs).

What one run does: it plays longgop_720p_10min.mp4 headlessly through the production parts. sub_audio::OutputRenderer renders real 512-frame callback blocks from the mixer at the 48 kHz sequence rate into a device that only offers 44.1 kHz, so every block goes through the sample-rate conversion, and publishes the AudioClock; sub_edit::playback::PlaybackScheduler follows that clock and picks the frame; sub_media::Decoder decodes forward to it, so the thing compared against the clock is a real decoded picture's PTS. Once per second of timeline the audible position is sampled against the frame on screen.

Drift definition: 'offset' is the audible position minus the displayed frame's PTS; 'drift' is how far that offset reaches outside the displayed frame's own display interval, so a correctly chosen frame reads zero and 'under one frame' is an exact comparison. Both are integers in milli-frames computed from RationalTime::as_seconds_fraction over i128 -- no floats, and fractional rates such as 23.976 stay exact.

Reporting: a new optional 'sync' section in the existing JSON report (report.rs SyncSection, schema_version unchanged because the field is purely additive) plus two summary lines. The run exits non-zero with the new stable code bench.drift_exceeded when max drift reaches a whole frame; a fixture that was never generated is a skipped section and exit 0, so the harness never fails a build on an environment problem.

CI: a Linux-only pair of steps in .github/workflows/ci.yml generates the long fixture (gen-fixtures.sh --long, which the existing fixture step deliberately skips) and runs the harness, uploading target/bench/av-sync.json as the av-sync-report artifact. Unlike the performance step this one is allowed to fail the job, which is how 'under one frame on Linux' is asserted.

Note on the fixture: longgop_720p_10min.mp4 is video-only, so the master is the sequence transport running silence over the fixture's timeline. That is the clock relationship the criterion is about; the fixture's pictures are what is decoded and compared. Documented in the module header and in docs/PERFORMANCE.md.

Verification (Linux/WSL2, GStreamer from the local prefix, no GPU, debug build):
- cargo fmt --all --check clean.
- cargo clippy --workspace --all-targets -- -D warnings clean.
- cargo test -p subordinate-bench: 41 tests green (24 unit tests including the new drift arithmetic, sample cadence, report and argument-parsing tests, plus tests/av_sync_harness.rs which runs the binary end to end and asserts either a measured run inside one frame or a skipped run that says why).
- Full ten-minute run against the real fixture: 599 s played, 599 one-second samples, 14998 frames shown, 0 dropped, max drift 0.000 frames, max offset 0.288 frames (worst sample at 113 s). Report and table recorded in docs/PERFORMANCE.md.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added an A/V sync and drift harness as a mode of the benchmark binary: 'subordinate-bench --sync' plays the ten-minute long fixture headlessly, driving the real audio output renderer (48 kHz sequence resampled to a 44.1 kHz device) so its AudioClock is the master, following it with PlaybackScheduler and decoding forward to the chosen frame, then sampling the audible position against the displayed frame's PTS once per second. Maximum drift is reported in milli-frames by exact integer arithmetic in a new 'sync' section of the JSON report and on stdout, and the run exits non-zero with bench.drift_exceeded at a whole frame, which a new Linux-only CI step (which also generates the long fixture) uses to assert the criterion. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings, cargo test -p subordinate-bench (41 tests, including an end-to-end run of the binary) and a full ten-minute run: 599 samples, 14998 frames, 0 dropped, max drift 0.000 frames -- recorded in docs/PERFORMANCE.md.
<!-- SECTION:FINAL_SUMMARY:END -->
