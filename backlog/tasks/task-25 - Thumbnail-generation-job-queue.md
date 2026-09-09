---
id: TASK-25
title: Thumbnail generation job queue
status: In Progress
assignee:
  - '@opus-task-25'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 14:02'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-15
  - TASK-12
references:
  - docs/PLAN.md
priority: medium
ordinal: 46000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Bins and timeline strips need thumbnails without blocking editing (§5.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A background job service with priorities, cancellation and progress events
- [ ] #2 Thumbnail job produces a strip of N frames per media item into the sidecar dir as compressed images
- [ ] #3 Jobs resume after restart and skip already-generated files
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-core: new jobs module — a background JobService with a priority queue (Interactive > Normal > Background, FIFO within a priority), N worker threads, per-job CancelToken, JobHandle (state, cancel, wait) and JobEvent progress/lifecycle events delivered to subscribers. Pure std, no new deps, fully unit tested.
2. Consolidate cancellation: sub_media::CancelToken becomes a re-export of sub_core::jobs::CancelToken so index jobs and thumbnail jobs share one type.
3. sub-media: new thumbnail module — ThumbnailOptions { count, max_width, quality }, exact RationalTime timestamp layout (duration * (2i+1) / 2N, no floats), seek+decode each frame with the existing Decoder, NV12->RGB with a box downscale, JPEG encode (pure-Rust jpeg-encoder crate), written into the sidecar dir as <content-hash>.thumb.NNN.jpg plus a <content-hash>.thumbs.json manifest (schema version, source hash, options, per-frame pts).
4. Resume/skip: generation reads the manifest first; a strip whose manifest matches the hash+options+version and whose files all exist is returned without decoding, and a partially generated strip only produces the missing frames, so a cancelled or interrupted job resumes after restart. Stale/corrupt manifests are rebuilt, never fatal.
5. Stable codes: media.thumbnail_failed for a strip that cannot be produced; cancellation stays core.cancelled.
6. Verify: cargo fmt --all --check, clippy pedantic -D warnings and cargo test -p sub-core. sub-media cannot be compiled in this environment (no GStreamer, no sudo); say so in the notes and leave anything it alone proves unchecked unless the pure logic can be verified out of tree.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
AC 1 (checked): sub_core::jobs is the background job service. JobService owns a worker pool and a priority queue (Interactive > Normal > Background, FIFO within a priority via the submission sequence); submit() returns a JobHandle at once; every job body gets a JobContext with is_cancelled/check/progress and a CancelToken it can hand deeper down; JobEvent (Queued, Started, Progress{done,total}, Finished{outcome}) is fanned out to every subscribe() receiver in order; cancel_all and Drop cancel both queued and in-flight work; a panicking job becomes core.internal and does not take its worker with it. Proven by 14 unit tests in crates/sub-core/src/jobs.rs, including the exact run order under a held worker, a queued job that is cancelled never running, a running job reporting core.cancelled, both subscribers receiving the identical event sequence, and drop cancelling outstanding work: cargo test -p sub-core = 27 passed + 3 doc-tests, cargo clippy -p sub-core --all-targets -- -D warnings clean.

Cancellation is now one type: sub_media::CancelToken is a re-export of sub_core::CancelToken rather than a second flag of the same shape, so an index build, a thumbnail strip and any future job share tokens.

AC 2 and 3 (implemented, NOT checked here): crates/sub-media/src/thumbnail.rs generates the strip. ThumbnailOptions{count, max_width, quality} is part of a strip's identity, so the sidecar files are <content-hash>.t<count>w<width>q<quality>.<nnn>.jpg beside a .thumbs.json manifest (schema version 1, source hash, options, per-frame pts/size); the sample times are exact integer nanoseconds, duration*(2i+1)/(2N), never a float; each picture is seeked with the existing Decoder (software decode, deliberately), box-downscaled from NV12/I420 to BT.709 RGB and JPEG-encoded with the pure-Rust jpeg-encoder crate, written to a .tmp and renamed so a reader never sees half a picture. Resume: generation loads the manifest first and returns without probing or decoding when it is complete, and otherwise skips every planned frame whose file is already on disk, so an interrupted run costs only its missing pictures. spawn_thumbnail_job() puts all of that on the JobService and reports (done, total) progress per picture.

Why they stay unchecked: this environment has no GStreamer and no sudo (pkg-config finds no gstreamer-1.0), so sub-media cannot be compiled, let alone run against the fixtures — cargo clippy --workspace fails in gstreamer-sys's build script before reaching any code of mine. What was proven here: the whole GStreamer-free half of the module (strip time layout, thumbnail sizing, option validation, file naming, the NV12 box downscale and colour conversion, JPEG encoding, manifest round-trip and the rejection of a stale, foreign or corrupt manifest) was compiled and run out of tree against stub decode/probe modules with the same signatures — 10 tests passing, and clippy::pedantic plus missing_docs clean over that source. The end-to-end evidence for AC 2 and AC 3 is crates/sub-media/tests/thumbnail_fixtures.rs, which asserts a strip of four real JPEGs of the right size in the sidecar dir, that a second run leaves the files' mtimes untouched, that a run missing its manifest and last two pictures regenerates exactly those two, and that a cancelled job reports core.cancelled and leaves no complete strip. Those tests need a machine or CI runner with GStreamer and generated fixtures; they skip themselves without them.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the background job service (sub_core::jobs: priority queue, worker pool, cooperative cancellation, progress and lifecycle events) and the thumbnail strip built on it (sub_media::thumbnail: N exactly spaced pictures per media item, JPEG-encoded into the project sidecar dir beside a manifest, keyed by content hash and options so an interrupted or repeated run only produces what is missing). Verified with cargo fmt --all --check, cargo clippy -p sub-core --all-targets -- -D warnings and cargo test -p sub-core (27 tests + 3 doc-tests, all green), plus an out-of-tree compile and run of the GStreamer-free half of the thumbnail module (10 tests, clippy pedantic clean). AC 1 is proven; AC 2 and 3 are implemented with fixture-backed tests in crates/sub-media/tests/thumbnail_fixtures.rs but stay unchecked because this machine has no GStreamer, so sub-media cannot be built or run here — they need CI or a machine with the runtime.
<!-- SECTION:FINAL_SUMMARY:END -->
