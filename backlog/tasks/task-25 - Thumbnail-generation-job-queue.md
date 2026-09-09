---
id: TASK-25
title: Thumbnail generation job queue
status: Done
assignee:
  - '@opus-task-25'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 15:42'
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
- [x] #2 Thumbnail job produces a strip of N frames per media item into the sidecar dir as compressed images
- [x] #3 Jobs resume after restart and skip already-generated files
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

Requeued 2026-09-09 by supervisor: the earlier worker could not build against GStreamer because the scratch prefix was missing. A stable prefix now exists: source /home/admin2/.cache/subordinate/env-gst.sh before cargo. Continue from the merged partial implementation on main; only the unchecked criteria remain.

Verification pass 2026-09-09 (worker on branch task/task-25), with the stable local GStreamer prefix (/home/admin2/.cache/subordinate/env-gst.sh) that the earlier run lacked. sub-media now compiles and runs here, so AC 2 and AC 3 have real evidence and are checked.

- cargo build -p sub-media --tests: clean (gstreamer 0.25 stack + jpeg-encoder link against the local prefix).
- cargo test -p sub-media --test thumbnail_fixtures against the generated fixtures (SUB_FIXTURES_DIR=/home/admin2/Subordinate/fixtures): 4 passed, 0 skipped (checked --nocapture for the 'skipping: no fixture' line; it is absent, so the assertions really ran against bars_1080p_h264.mp4).
  - AC 2: a_strip_of_n_compressed_pictures_lands_in_the_sidecar_directory — four complete JPEGs (SOI..EOI) of 160x90 in the sidecar dir, strictly increasing pts inside the clip, and the strip reloads from the manifest alone equal to the generated one.
  - AC 3: a_second_run_reuses_the_pictures_and_an_interrupted_one_finishes_the_rest — a repeat run leaves the first picture's mtime untouched (nothing decoded); after deleting the manifest and the last two pictures, generation reproduces exactly those two byte-for-byte and still does not rewrite the first two. That is the resume-after-restart and skip-already-generated behaviour.
  - AC 1 (already checked): the_job_service_runs_a_strip_off_the_calling_thread_and_reports_progress asserts the exact (1,4)..(4,4) progress sequence, and a_cancelled_thumbnail_job_stops_and_reports_cancellation cancels mid-strip and gets core.cancelled with no complete strip left behind.
- cargo test -p sub-core: 27 passed. cargo test -p sub-media: 79 unit + all integration suites pass except one PRE-EXISTING, unrelated failure — probe_fixtures::every_fixture_probes_into_the_shape_the_manifest_promises fails with 'vfr_60_30.mkv: duration 5983333333 ns, manifest says 6000000000 ns'. That is TASK-13 probe/fixture-manifest tolerance on the VFR clip, untouched by this task; no thumbnail code is involved.
- cargo fmt --all --check: clean. cargo clippy --workspace --all-targets -- -D warnings: clean across the whole workspace (it now gets past gstreamer-sys).

No code changes were needed in this pass: the implementation merged from the earlier attempt is correct as written; what was missing was only the ability to build and run it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Background job service plus thumbnail strip generation. sub_core::jobs is a JobService with a priority queue (Interactive > Normal > Background, FIFO within a priority), a worker pool, cooperative CancelToken cancellation shared with sub-media, and Queued/Started/Progress/Finished events fanned out to subscribers. sub_media::thumbnail generates N exactly spaced pictures per media item — sample times are integer RationalTime, duration*(2i+1)/(2N), never floats — seeking with the existing Decoder, box-downscaling NV12/I420 to BT.709 RGB and JPEG-encoding into the sidecar dir under names keyed by content hash and options, beside a versioned manifest; generation loads the manifest first and otherwise skips every picture already on disk, so a repeated run decodes nothing and an interrupted one costs only its missing frames.

Verified on a machine with the GStreamer runtime: cargo fmt --all --check and cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-core 27 passed; cargo test -p sub-media --test thumbnail_fixtures 4 passed against the real fixtures with no skips, covering the strip of JPEGs and its manifest round-trip (AC 2), the untouched-mtime repeat run and the resume that regenerates exactly the two deleted pictures (AC 3), and the off-thread job's exact progress sequence and mid-strip cancellation to core.cancelled (AC 1). All three acceptance criteria checked. One unrelated pre-existing failure remains in probe_fixtures (vfr_60_30.mkv duration tolerance, TASK-13 territory), not touched here.
<!-- SECTION:FINAL_SUMMARY:END -->
