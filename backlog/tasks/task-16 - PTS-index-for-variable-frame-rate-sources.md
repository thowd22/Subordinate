---
id: TASK-16
title: PTS index for variable-frame-rate sources
status: Done
assignee:
  - '@opus-task-16'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 15:48'
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
- [x] #2 Seek and frame stepping on the VFR fixture use the index and land on the correct frame
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

8. Requeue: reproduce the AC-2 blocker by dumping parsed and decoded PTS of vfr_60_30.mkv with gst-launch.
9. Find the root cause in the fixture generator, not the index: x264's B-frame reorder delay assumes one frame duration and misstamps the pictures either side of the 30 to 60 fps change.
10. Add bframes=0 to the VFR fixture pipeline in scripts/gen-fixtures.sh and scripts/gen-fixtures.ps1, kept in sync, and regenerate the fixture.
11. Drop the known-fixture-defect caveat from crates/sub-media/tests/index_fixtures.rs, cover the whole VFR file with index-driven seek and stepping judged by bit-identical pictures, and add a guard test that the fixture gives every picture its own instant.
12. Verify fmt, clippy -D warnings and cargo test -p sub-media -p sub-test-support with the GStreamer env, then check AC 2.
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

Requeue 2026-09-09 by @opus-task-16: AC 2 is now proven, and the fixture defect that blocked it is fixed at its source.

Diagnosis. Dumping the fixture's timestamps with gst-launch showed the damage came out of the encoder, not the decoder: in storage order the old vfr_60_30.mkv carried 5.933 s and 5.967 s where the second segment's first two pictures, 3.000 s and 3.017 s, belonged; those two values then appeared twice in the file, and avdec_h264 consequently stamped every picture after the frame-rate change 5.967 s. Root cause is x264's B-frame reorder delay, which assumes a single frame duration: at the 30 to 60 fps change the pictures either side of it are handed each other's timestamps. Regenerating with bframes=0 removes it entirely - 270 pictures, 270 distinct strictly increasing timestamps, both the 33.3 ms and 16.6 ms spacing present. Declaring the relabelled caps framerate=0/1 instead was tried and is neither sufficient, since B-frames still break it, nor necessary, and it costs the container the last frame's duration and so the probed clip duration, so the caps are left as they were.

Change. scripts/gen-fixtures.sh and scripts/gen-fixtures.ps1, kept in sync, add bframes=0 to the vfr_60_30.mkv encoder only, with a comment saying why; the constant-rate clips keep their B-frames. No production code changed - the index, IndexedDecoder and the sidecar cache were already correct.

Tests. crates/sub-media/tests/index_fixtures.rs drops the known-fixture-defect caveat and the VFR_LAST_SOUND_FRAME cap. index_driven_seek_and_stepping_land_on_the_correct_frame now seeks over the whole VFR file - frames 0, 1, 29, 89, 90, 91, 150, 200, 269 and 12 - and steps twelve frames across the rate change, every picture judged bit-identical to the one a straight decode produces at that frame number. lookup_by_time_round_trips_through_every_frame now asserts the exact frame number rather than only its timestamp. A new guard test, the_vfr_fixture_gives_every_picture_its_own_instant, asserts 270 entries, strictly increasing timestamps and the 33.3 and 16.6 ms spacings either side of frame 90, so a regenerated fixture that regresses this way fails loudly instead of being worked around.

Verification, with the stable GStreamer prefix, SUB_FIXTURES_DIR=/home/admin2/Subordinate/fixtures and the fixture regenerated from the fixed pipeline: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-media -p sub-test-support all green, 79 unit tests plus every integration suite, index_fixtures 6 of 6 and probe_fixtures 7 of 7, the latter including the 6 s duration and VFR-detection assertions. bash -n on the shell generator passes. Note for CI: scripts/gen-fixtures.sh cannot run end to end on this machine because timecodestamper needs libltc, which the extracted GStreamer prefix lacks; the vfr_60_30.mkv fixture used here was produced by running that script's pipeline verbatim. CI regenerates every fixture, so the change is exercised there.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed the last open criterion of the PTS index work: index-driven seek and frame stepping now land on the correct frame across the whole variable-frame-rate fixture. The blocker was the fixture, not the index - the old vfr_60_30.mkv came out of x264enc with the pictures either side of the 30 to 60 fps change carrying each other's timestamps, because x264's B-frame reorder delay assumes a single frame duration. scripts/gen-fixtures.sh and scripts/gen-fixtures.ps1 now pass bframes=0 for that one fixture, which yields 270 pictures with 270 distinct, strictly increasing timestamps and both the 33.3 ms and 16.6 ms spacings; no production code needed to change. crates/sub-media/tests/index_fixtures.rs drops its known-defect caveat and now seeks to frames 0, 1, 29, 89, 90, 91, 150, 200 and 269 and steps twelve frames across the rate change, judging every picture bit-identical to a straight decode, with a new guard test that fails loudly if a regenerated fixture ever regresses. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-media -p sub-test-support, all green including index_fixtures 6 of 6 and probe_fixtures 7 of 7. All three acceptance criteria are checked.
<!-- SECTION:FINAL_SUMMARY:END -->
