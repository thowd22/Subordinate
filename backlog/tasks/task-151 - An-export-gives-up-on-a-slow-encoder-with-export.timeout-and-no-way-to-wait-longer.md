---
id: TASK-151
title: >-
  An export gives up on a slow encoder with export.timeout and no way to wait
  longer
status: Done
assignee:
  - '@codex'
created_date: '2026-09-12 12:00'
updated_date: '2026-09-12 18:10'
labels:
  - export
  - bug
dependencies: []
ordinal: 171000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The export matrix (TASK-143) renders eight frames of the sample project with av1enc, libaom AV1 encoder. On a free hosted four-core runner that takes 82 seconds and passes; on the T4 instance, which is also four cores but busier, the same eight frames end in "export.timeout: the export pipeline never finished writing" (run 34689001087) and no file. Nothing is wrong with the encoder or the file it would have written - it is simply slower than the pipeline fixed patience, and libaom at stock cpu-used is seconds to minutes a frame at any resolution a user would actually export.

So an export that would have succeeded is abandoned, and the user is told the pipeline never finished writing rather than that it ran out of time. Two things are missing: the timeout should be about progress rather than about total elapsed time, since an encoder that is producing a frame a minute is working; and where a hard limit is still wanted it belongs in the export request rather than being a constant.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An export that is still making progress is not abandoned, however slow the encoder is
- [x] #2 An export that has genuinely stopped is failed with an error that says so and names the element it was waiting on
- [x] #3 A test covers a deliberately slow encoder finishing an export that the current fixed timeout would abandon
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Keep TASK-146 bus-aware bounded pushes and planar conversion. Apply request stall_timeout_ms to push and EOS waits, resetting patience for queue, file, position or encoder-output progress. 2. Honor an optional timeout_ms cap on final EOS draining even when progress continues. 3. Preserve stable timeout reason and element details. 4. Replace machine-dependent noisy AV1 tests with an x264 stream delayed by a pad probe, proving continuous progress survives a short patience window and an explicit deadline still wins. 5. Run combined export tests and clippy locally.

Post-integration review: observe encoder EOS to distinguish an internally stalled encoder from a muxer after appsrc has drained; add a deterministic regression holding encoder EOS after the queue empties.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Reproduced the shape from the report: the only time budget is EOS_TIMEOUT_SECONDS = 120 in crates/sub-export/src/pipeline.rs, spent entirely in wait_for_eos after both appsrc streams have ended, so a slow encoder draining its queue is indistinguishable from a dead one.

Replaced the expensive 240-frame AV1 test with deterministic 50 ms delays on 24 x264 input buffers. Continuous progress completes beyond a 300 ms patience window; a 400 ms explicit drain limit fails despite ongoing progress and names x264enc. Encoded-output pad probes additionally prevent buffered muxer output from hiding encoder progress. Existing starved-branch regression checks stopped pushes and cleanup. timeout_ms intentionally caps final EOS draining, while stall_timeout_ms governs both pushes and EOS.

Post-integration review now observes each encoder output EOS. An empty appsrc no longer falsely attributes an encoder flush stall to the muxer. A deterministic test holds x264 input EOS after the queue drains and verifies export.timeout names x264enc; all 33 pipeline unit tests and clippy across export targets pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Replaced fixed EOS patience with progress-aware request settings and integrated them with bus-aware push backpressure. Encoder output, queue levels, file growth and position reset patience; explicit final-drain limits still apply. Combined sub-export tests passed, including deterministic delayed x264 completion, deadline failure naming x264enc, timeout verdicts, and the existing starved-branch regression; clippy passed with warnings denied.
<!-- SECTION:FINAL_SUMMARY:END -->
