---
id: TASK-16
title: PTS index for variable-frame-rate sources
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 15:09'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-15
references:
  - docs/PLAN.md
priority: medium
ordinal: 37000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
VFR is a known source of proxy and seek bugs (§9); an explicit index avoids assuming constant frame duration.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Index maps frame number to PTS and keyframe flags, built lazily on first access and cached in the sidecar dir
- [ ] #2 Seek and frame stepping on the VFR fixture use the index and land on the correct frame
- [x] #3 Index builds are cancellable background jobs
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New crates/sub-media/src/index.rs: PtsIndex (frame number -> exact RationalTime PTS + keyframe flag) built from a parse-only parsebin pipeline that records buffer PTS and the DELTA_UNIT flag; sorted into presentation order.
2. Sidecar cache: PtsIndex::load_or_build(path, sidecar_dir) writes/reads <content-hash>.ptsindex.json under the sidecar dir, validated by ContentHash plus a schema version; a stale or corrupt cache is rebuilt, never fatal.
3. Lazy access: LazyPtsIndex holds the source path and cache dir and builds on first get(), memoising the Arc<PtsIndex>.
4. Cancellable background job: CancelToken (Arc<AtomicBool>) checked per buffer inside the pipeline probe; IndexJob::spawn runs a build on a worker thread, cancel()/join() return core.cancelled promptly.
5. Index-driven seek and stepping: IndexedDecoder wraps Decoder and seeks to the exact indexed PTS of a frame number, step_by(delta) walks frames without assuming constant duration.
6. New stable code media.index_failed. Unit tests for lookup/keyframe/cache/cancel plus an integration test over the vfr_60_30.mkv fixture (skips when fixtures absent).
7. Verify fmt, clippy -D warnings, cargo test -p sub-media with the GStreamer env.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-media/src/index.rs (new module, exported from lib.rs):

- PtsIndex: one entry per picture in presentation order, each an exact nanosecond PTS (RationalTime, no floats) plus a keyframe flag. Built by a parse-only filesrc ! parsebin pipeline that records every video buffer's PTS and its DELTA_UNIT flag, then sorts into presentation order. Lookups: pts(frame), is_keyframe(frame), duration_of(frame) (the gap to the next picture — the value a VFR source varies), frame_at(time) (floor), frame_at_or_after(time) (ceil), keyframe_at_or_before(frame). All comparisons are exact integer nanoseconds.
- Sidecar cache: load_or_build(path, cache_dir, cancel) writes <content-hash>.ptsindex.json (flat pts/keyframe arrays, schema version 1) into the project's sidecar dir, keyed and validated by sub_model::ContentHash. A cache for other bytes, another schema version, or a corrupt one is rebuilt rather than trusted; a cache that cannot be written is logged, not fatal (a read-only sidecar must not stop an edit). Written to a .tmp and renamed so a reader never sees half a file.
- LazyPtsIndex: builds on first get(), memoises the Arc; is_built() distinguishes a cheap access from one that will parse.
- CancelToken (Arc<AtomicBool>) + IndexJob: build runs on a worker thread; cancel() is checked between 50 ms bus polls and on every buffer, join() returns core.cancelled, Drop cancels without blocking.
- IndexedDecoder: a Decoder driven by frame numbers — seek_to_frame(n) seeks to the index's timestamp for n, step(delta) walks frames; nothing multiplies a frame number by a frame duration.
- New stable code media.index_failed; cancellation reuses core.cancelled.

Verification (this machine, GStreamer from the scratchpad prefix, SUB_FIXTURES_DIR=/home/admin2/Subordinate/fixtures):
- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-media: 52 unit + 9 + 9 + 5 (new index_fixtures) + 7 + 7 integration tests and 6 doc-tests, all pass.

AC 1 (checked): index_fixtures::index_records_more_than_one_frame_duration_for_a_vfr_source and lookup_by_time_round_trips_through_every_frame prove the frame->PTS/keyframe mapping over vfr_60_30.mkv (270 entries, both a 30 fps and a 60 fps spacing present); the_index_is_built_lazily_and_cached_in_the_sidecar_dir proves nothing is parsed before the first access, that the built index is memoised, that <hash>.ptsindex.json appears in the cache dir, that a second lazy index reads it back identically, and that a corrupt cache is rebuilt.
AC 3 (checked): an_index_build_is_a_cancellable_background_job proves a pre-set token fails the build with core.cancelled, that a spawned job stops promptly when cancelled, and that a job left to run yields the same index as a direct build.
AC 2 (NOT checked): index-driven seek and stepping are implemented and proven by picture comparison — every frame reached is bit-identical to the picture a straight decode produces at that frame number — over the whole of bars_1080p_h264.mp4 (control) and over frames 0..89 of vfr_60_30.mkv, but they cannot be proven over the rest of the VFR fixture because that fixture is malformed. Evidence: parsed in storage order the file's second-segment IDR carries PTS 5.933 s while the pictures after it carry 3.033 s onward, and avdec_h264 consequently stamps every decoded picture after the frame-rate change 5.967 s (both software and default decoder, gstreamer 1.28 here). Any seek past frame 90 therefore stops on the first picture whose (wrong) timestamp is >= the target. The pictures decode in the right order; only their timestamps are wrong, so no index can name them by time. Root cause is the generator pipeline in scripts/gen-fixtures.sh, which lies to x264enc about the frame rate (capssetter replace=true framerate=60/1) while feeding it 30 fps timestamps; x264's output PTS assignment does not survive that. Fixing the fixture generator is TASK-10 territory and outside this task's scope, so AC 2 is left unchecked and needs a follow-up decision from the user.

Requeued 2026-09-09 by supervisor: implementation is merged on main with criteria 1 and 3 checked. Remaining: criterion 2 (seek and frame stepping on the VFR fixture use the index and land on the correct frame). Use the stable GStreamer env: source /home/admin2/.cache/subordinate/env-gst.sh; fixtures come from scripts/gen-fixtures.sh which works with that env.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the PTS index for variable-frame-rate sources: sub_media::index gives PtsIndex (frame number -> exact nanosecond PTS plus keyframe flag, built from a parse-only parsebin pipeline and sorted into presentation order), a sidecar JSON cache keyed by ContentHash that is rebuilt rather than trusted when stale or corrupt, LazyPtsIndex for build-on-first-access, a cancellable IndexJob worker (core.cancelled), IndexedDecoder for frame-numbered seek and stepping, and the stable code media.index_failed. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-media (all green), including a new tests/index_fixtures.rs that judges index-driven seeks and steps by bit-identical picture comparison. AC 1 and 3 are proven; AC 2 is proven for the constant-rate control fixture and for frames 0..89 of vfr_60_30.mkv only, because that fixture carries misassigned timestamps in its second segment (see notes) — it stays unchecked pending a decision on the fixture generator.
<!-- SECTION:FINAL_SUMMARY:END -->
