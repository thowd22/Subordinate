---
id: TASK-151
title: >-
  An export gives up on a slow encoder with export.timeout and no way to wait
  longer
status: In Progress
assignee:
  - '@codex'
created_date: '2026-09-12 12:00'
updated_date: '2026-09-12 17:55'
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
- [ ] #1 An export that is still making progress is not abandoned, however slow the encoder is
- [ ] #2 An export that has genuinely stopped is failed with an error that says so and names the element it was waiting on
- [ ] #3 A test covers a deliberately slow encoder finishing an export that the current fixed timeout would abandon
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Replace the fixed 120 s EOS wait in ExportPipeline::wait_for_eos with a progress watch: poll the bus in short slices, sample a progress mark (bytes on disk, both appsrc queue levels, pipeline position) and reset the patience window whenever it moves.
2. Move the limits into the export request: ExportSettings gains stall_timeout_seconds (default 120) and an optional hard timeout_seconds, both serde-defaulted so existing requests deserialise, with builders and validation.
3. Fail a genuinely stopped export with export.timeout naming the element it was waiting on (the video/audio encoder whose queue is still full, else the muxer), and separate that message from the hard-limit message.
4. Tests: unit tests for the stall verdict, the element naming and validation; an integration test that exports through av1enc (the encoder from run 34689001087) with a one-second stall window and shows it finishes, plus one showing a hard limit fails with the element named.
5. Verify with cargo fmt, clippy pedantic and cargo test -p sub-export under the GStreamer env.

Resume preserved agent implementation; integrate with TASK-146 and the other export fixes on a shared export branch, then validate locally without AWS.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Reproduced the shape from the report: the only time budget is EOS_TIMEOUT_SECONDS = 120 in crates/sub-export/src/pipeline.rs, spent entirely in wait_for_eos after both appsrc streams have ended, so a slow encoder draining its queue is indistinguishable from a dead one.
<!-- SECTION:NOTES:END -->
