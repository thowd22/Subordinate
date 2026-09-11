---
id: TASK-61
title: 'Export job with progress, ETA, cancel and error reporting'
status: Done
assignee:
  - '@opus-task-61'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 09:57'
labels:
  - export
milestone: m-4
dependencies:
  - TASK-59
references:
  - docs/PLAN.md
priority: high
ordinal: 82000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Long renders need feedback and must be cancellable without leaving a broken file.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Export runs as a job with frames-done, ETA and encoder stats events
- [x] #2 Cancel stops the pipeline and removes the partial file
- [x] #3 Failures surface the GStreamer error with the element name
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-export/src/job.rs: ExportJob (path, settings, elements, total frames, cancel token, progress interval), ExportProgress (frames done/total, elapsed, encode rate, ETA, percent) and EncoderStats (encoder/muxer element names, video+audio frames, bytes written, encoded media duration as RationalTime), all computed with integer arithmetic, no floats.
2. Emit ExportEvent::{Started, Progress, Finished, Cancelled, Failed} to a caller callback; ExportJob::run drives the push loop, checking the cancel token between frames.
3. Cancel: add ExportPipeline::abort(), which sets the pipeline to Null and removes the part-written file; the job returns core.cancelled and reports Cancelled.
4. Error reporting: carry the failing GStreamer element name (message src path_string + name) into SubError details on bus errors and refused pushes, so a failure names the element.
5. spawn_export_job(JobService, ...) mirrors spawn_proxy_job: returns an ExportJobHandle with cancel, a Receiver<ExportEvent> and the ExportReport, bridging progress onto JobEvent::Progress.
6. Unit tests for ETA/rate/percent maths and event ordering plus an integration test that cancels a real export and asserts the partial file is gone; fmt, clippy, cargo test -p sub-export.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
New crates/sub-export/src/job.rs holds the job layer around ExportPipeline.

- ExportJob is a builder (total frames, CancelToken, progress interval) whose run() drives the push loop and reports ExportEvent::{Started, Progress, Finished, Cancelled, Failed}. spawn_export_job() puts one on a sub-core JobService, mirroring spawn_proxy_job: frame counts go out as JobEvent::Progress, and the richer ExportEvent stream (which JobEvent has no room for) goes to ExportJobHandle::events().
- ExportProgress carries frames done/total, elapsed, ETA, encode rate and percentage; EncoderStats carries the encoder/muxer element names, video and audio frame counts, bytes on disk and the media time written as a RationalTime, with an exact bits_per_second() from its rational duration. Every derived number is u128 integer arithmetic; rates and percentages are carried in thousandths so no float reaches a report, a log line or an agent.
- Cancellation is checked before every frame. A cancelled or failed export calls the new ExportPipeline::abort(), which sets the pipeline to Null and deletes the part-written file (a muxer that never saw end of stream leaves no index and, for the ISO containers, no moov atom). ExportPipeline::finish() now also tears the pipeline down on its error path instead of leaving elements running behind the error.
- Error reporting: element_error()/with_bus_error() in pipeline.rs put the failing element's name and object path into the SubError message and details for bus errors, refused pushes and refused state changes. Before this a failing export said only 'the export pipeline failed'.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-export green (34 unit, 6 new export_job integration tests, 4 existing round-trip tests, 3 doctests), with GStreamer from the local gstroot. The integration tests run real x264enc/matroskamux pipelines here, so none of them skipped: one asserts the per-frame progress sequence, ETA and stats; one asserts a cancel leaves no file and reports Cancelled; one writes into a missing directory and asserts the error names filesink; two exercise spawn_export_job including cancelling through the handle.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the export job layer (crates/sub-export/src/job.rs): ExportJob drives the pipeline frame by frame, emitting ExportEvent progress snapshots with frames done, an integer-arithmetic ETA, encode rate and EncoderStats (elements, frame counts, bytes written, media time as RationalTime), and spawn_export_job runs one on a JobService. Cancellation is checked between frames and aborts through the new ExportPipeline::abort(), which stops the pipeline and deletes the part-written file; failures now carry the failing GStreamer element's name and object path in the SubError message and details. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-export, including six new integration tests that run real x264enc/matroskamux exports covering progress, cancellation and the named-element failure.
<!-- SECTION:FINAL_SUMMARY:END -->
