---
id: TASK-26
title: Benchmark harness for scrub and playback performance
status: In Progress
assignee:
  - '@opus-task-26'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 10:11'
labels:
  - test
  - media
milestone: m-1
dependencies:
  - TASK-23
references:
  - docs/PLAN.md
priority: medium
ordinal: 47000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Phase 1 exit criteria are numeric; a repeatable harness prevents regressions.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A criterion or custom benchmark measures decode-to-texture latency and sustained scrub fps on the 1080p and 4K fixtures
- [ ] #2 Results are written as JSON and summarised in CI logs on Linux
- [x] #3 Baseline numbers are recorded in docs/PERFORMANCE.md
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add bins/subordinate-bench: a custom (non-criterion) harness measuring decode-to-texture latency and sustained scrub fps on the 1080p and 4K fixtures, reusing sub-media Decoder and sub-render Nv12Converter (the real upload path).
2. Integer-only maths: every duration an exact nanosecond count, every rate a milli-fps integer; percentiles p50/p95/max plus mean per stage (decode, upload+convert, total).
3. Playback scenario = sequential decode of N frames; scrub scenario = deterministic zig-zag seeks across the file, both timed to GPU completion.
4. Graceful degradation: a missing fixture or a machine with no wgpu adapter records a skipped/decode-only scenario instead of failing, so the harness runs anywhere.
5. Write results as JSON (--out, default target/bench/perf.json) and print a human summary for CI logs; unit tests cover stats, argument parsing and JSON shape.
6. CI: run the harness on Linux after the test step, tee the summary into the job log and step summary, upload the JSON as an artifact.
7. Record baseline numbers and how to reproduce them in docs/PERFORMANCE.md.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added bins/subordinate-bench, a custom harness (no criterion dependency: the run is a handful of seconds and criterion's statistics assume a cheap, repeatable microbenchmark, which decoding 4K is not).

What it measures, through the production paths (sub_media::Decoder plus sub_render::Nv12Converter, the same NV12 upload and YUV-to-RGB pass the viewer uses):
- playback: next_frame then upload-and-convert, waited to GPU completion;
- scrub: seek_to a fresh position then the same upload. Scrub targets alternate head/tail and work inward (run::scrub_targets), so a run covers the whole clip and every step is a real keyframe seek rather than the decode-forward playback gets free.
Per stage it reports count/mean/min/p50/p95/max and a sustained rate. All durations are exact nanosecond counts and all rates are milli-fps integers; no timing value is a float. Errors are SubError with new stable bench.* codes (bad_argument, report_unwritable, frame_layout, gpu_lost), and RenderError keeps its own render.* code when it is wrapped.

Degradation instead of failure: a missing fixture becomes a skipped scenario with the reason recorded, and a machine with no wgpu adapter measures decode alone and marks the run '[decode only: no GPU]'. Both were exercised here (--fixtures /nonexistent, --no-gpu) and both exit 0.

Verification on this machine (WSL2, Ryzen 9 9900X, Mesa lavapipe software Vulkan, GStreamer 1.24, avdec_h264):
- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p subordinate-bench: 23 unit tests pass (percentiles, rate maths, scrub-target placement, argument parsing, JSON round-trip and the skipped/decode-only summary lines).
- Two release runs of the harness itself against the real 1080p and 4K fixtures; the second (idle machine) is the baseline table in docs/PERFORMANCE.md, and the spread between the runs is about 10%.

AC #2 is half proven and left unchecked: the JSON report is written and its shape is covered by a test, but the CI half (the 'Scrub and playback benchmark (Linux)' step, its tee into the job log and $GITHUB_STEP_SUMMARY, and the perf-report artifact upload) cannot be exercised from this environment -- it needs a push to GitHub Actions, which this worktree must not do. The workflow YAML was parsed to confirm the steps land in the right place and the exact command was run locally.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added bins/subordinate-bench: a custom benchmark harness that measures decode-to-texture latency and sustained playback and scrub frame rates on the 1080p and 4K fixtures through the real Decoder and Nv12Converter paths, writes a versioned JSON report (--out, default target/bench/perf.json) and prints a log summary. Baselines and how to read them are recorded in docs/PERFORMANCE.md, with a pointer from docs/DEVELOPMENT.md, and CI gains a Linux-only step that runs the harness, tees the summary into the job log and step summary, and uploads the report as the perf-report artifact. Verified with cargo fmt, clippy -D warnings across the workspace, 23 unit tests, and two release runs against the real fixtures; AC #2 stays unchecked because the CI half cannot be exercised without pushing to GitHub Actions.
<!-- SECTION:FINAL_SUMMARY:END -->
