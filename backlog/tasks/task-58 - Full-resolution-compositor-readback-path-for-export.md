---
id: TASK-58
title: Full-resolution compositor readback path for export
status: In Progress
assignee:
  - '@opus-task-58'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 18:11'
labels:
  - render
  - export
milestone: m-4
dependencies:
  - TASK-39
references:
  - docs/PLAN.md
priority: high
ordinal: 79000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Export must never drop frames and needs frames on the CPU (or in GPU memory the encoder accepts).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 render_frame_to_buffer(sequence, time) returns an RGBA or NV12 buffer at sequence resolution using a staging buffer ring to overlap GPU and CPU work
- [ ] #2 Throughput on the 1080p fixture exceeds 60 fps on a discrete GPU (measured)
- [x] #3 Readback never runs on the UI thread
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-render/src/readback.rs: a StagingRing of reusable MAP_READ buffers plus FrameReadback, which owns a Compositor and pipelines copy_texture_to_buffer submissions across the ring so the CPU unpacks frame N while the GPU draws N+1.
2. Public API: FrameReadback::render_frame_to_buffer(sequence, time) -> Result<FrameBuffer, RenderError> for the one-shot case, and submit()/receive()/drain() for the pipelined export loop. FrameBuffer carries tightly packed RGBA rows at the sequence resolution plus the frame time and FrameSummary.
3. Add stable RenderError variants for readback failures (render.readback_ring_full, render.readback_pending, render.readback_failed) and export them from lib.rs.
4. Keep readback off the UI thread: FrameReadback is Send and documented as export-only; the UI keeps sampling Compositor::output with no copy. Prove it with a test that drives readback on a spawned worker thread.
5. Tests in crates/sub-render/tests/readback.rs: pixel equality with Compositor::read_rgba, row unpadding, ring reuse across resolutions, ordering of pipelined frames, error codes, and an #[ignore]d 1080p throughput measurement for AC2 hardware runs.
6. Verify with cargo fmt --check, clippy pedantic -D warnings and cargo test -p sub-render.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-render/src/readback.rs (new module, re-exported from lib.rs).

Design
- StagingRing owns depth reusable MAP_READ buffers (DEFAULT_DEPTH = 3). A slot only reallocates when a larger canvas arrives, and a frame in flight holds its own handle to the buffer it is waiting on, so a resize never disturbs it.
- FrameReadback owns the Compositor so nothing can resize the target under a frame in flight. submit() renders, records copy_texture_to_buffer into the next slot, submits and immediately asks for the map; receive() waits on that frame's own SubmissionIndex (not the newest submission, which would serialise the pipeline), unpacks the padded rows into a tightly packed RGBA Vec, unmaps and frees the slot. drain() finishes the tail of an export.
- render_frame_to_buffer(sequence, time, source) is the one-shot form named by AC #1; it refuses with render.readback_pending rather than silently returning an earlier frame's pixels when the ring is not empty. It takes the same &mut dyn FrameSource that Compositor::render does, since the graph deliberately does not decide where pictures come from.
- New stable RenderError variants: render.readback_ring_full, render.readback_pending, render.readback_failed. A full ring errors instead of overwriting a buffer, because export may not drop frames.
- Output is RGBA (the 'RGBA or NV12' half of AC #1) at the sequence resolution, tightly packed; FrameBuffer also carries the frame time and the FrameSummary so the encoder side can pair pixels with what was drawn. RGBA-to-NV12 encoding is left to the export pipeline (TASK-59).

AC #2 is left unchecked: this machine has no discrete GPU (software lavapipe only), so the 60 fps claim cannot be measured here. The measurement harness is in place as an #[ignore]d test, throughput_on_a_1080p_canvas in crates/sub-render/tests/readback.rs; run it with 'cargo test -p sub-render --test readback -- --ignored --nocapture' on GPU hardware. For the record it already reports 272.6 fps at 1080p on llvmpipe (LLVM 20.1.2) here.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-render green (7 readback integration tests plus unit tests, 1 ignored throughput test).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added a pipelined full-resolution readback path for export: crates/sub-render/src/readback.rs gives FrameReadback::render_frame_to_buffer for one-shot frames and submit/receive/drain for the export loop, both backed by a StagingRing of reusable MAP_READ buffers so the CPU unpacks frame N while the GPU draws N+1, plus three stable RenderError codes for a full ring, a busy one-shot read and a failed map. Verified with seven GPU integration tests in crates/sub-render/tests/readback.rs (pixels identical to Compositor::read_rgba, padded rows stripped, submission order preserved, ring-full and pending errors, resolution changes, and a run driven entirely from a worker thread) plus rustfmt and workspace clippy -D warnings. AC #2 stays unchecked: no discrete GPU is available here, so the ignored throughput_on_a_1080p_canvas harness is left for a hardware run.
<!-- SECTION:FINAL_SUMMARY:END -->
