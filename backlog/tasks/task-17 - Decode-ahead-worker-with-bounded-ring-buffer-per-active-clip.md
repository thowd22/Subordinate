---
id: TASK-17
title: Decode-ahead worker with bounded ring buffer per active clip
status: Done
assignee:
  - '@opus-task-17'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 05:19'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-15
references:
  - docs/PLAN.md
priority: high
ordinal: 38000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Playback must not decode on the compositor thread; a worker keeps a bounded number of frames ready (§4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A worker thread per active decoder keeps N frames ahead of the playhead, dropping the buffer on seek
- [x] #2 Backpressure: the worker blocks when the ring is full and never allocates per frame
- [x] #3 Metrics for buffer occupancy and decode time per frame are exposed via tracing
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-media/src/decode_ahead.rs: DecodeAhead handle owning a worker thread per Decoder.
2. Bounded ring: VecDeque preallocated to capacity, Mutex + two Condvars (space/filled); worker blocks when full, never allocates per frame.
3. Seek: consumer bumps a generation and clears the ring; worker re-seeks via Decoder::seek_to and discards stale frames.
4. Metrics: occupancy, decode time per frame, drops and seeks exposed via a stats snapshot plus tracing (trace per frame, debug on seek/eos).
5. Tests: unit tests for ring/backpressure/stats plus a fixture-gated integration test (tests/decode_ahead_fixtures.rs) matching decode-ahead output against a plain Decoder, skipping when fixtures are absent.
6. Verify fmt, clippy -D warnings, cargo test -p sub-media with the GStreamer env sourced.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-media/src/decode_ahead.rs: DecodeAhead owns one Decoder on a named worker thread ('sub-decode-ahead') and fills a VecDeque allocated once to DecodeAheadOptions::capacity (default 8). Consumer pops with next_frame(); the worker parks on a 'space' condvar when the ring is full (counted as backpressure_waits) and on end of stream, and wakes on a seek or on the handle being dropped. seek_to() clears the ring in place (keeping the reservation), bumps a generation counter and hands the target to the worker, so frames still in flight for the old generation are counted as dropped instead of delivered. Errors from a worker decode or seek are stored and returned by the next next_frame(); a poisoned lock becomes media.decode_failed rather than a panic. No new error codes were needed. All timing stays RationalTime; no floats.

Verification (Linux, GStreamer 1.28 from a local sysroot, software decoders, fixtures via SUB_FIXTURES_DIR): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-media = 90 tests passing across 8 targets. AC1 is proven by tests/decode_ahead_fixtures.rs: decode_ahead_delivers_the_same_frames_as_a_plain_decoder (125 frames, identical PTS and identical luma planes against a plain Decoder, occupancy never above capacity) and seeking_drops_the_buffer_and_resumes_at_the_target (buffered frames are counted as dropped, the next frame is the frame the plain decoder's seek returns, decoding continues in order). AC2 by a_paused_consumer_fills_the_ring_and_stalls_the_worker (a consumer that never pulls leaves occupancy exactly at capacity, frames_decoded exactly at capacity and backpressure_waits above zero, still true after a further 300 ms) plus the_ring_is_allocated_once_to_its_capacity, which asserts the VecDeque reservation is made at construction; pushes are debug_asserted to stay inside it and seek clears in place, so no per-frame allocation happens on this path. AC3 by tests/decode_ahead_tracing.rs, which installs a global subscriber and asserts a TRACE event per frame on target sub_media::decode_ahead carrying pts_ns, decode_us, occupancy and capacity, plus the DEBUG seek event; the same numbers are also readable synchronously through DecodeAhead::stats().
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added sub_media::decode_ahead: a DecodeAhead handle that runs one Decoder on its own worker thread behind a bounded ring buffer, so playback never decodes on the compositor thread (docs/PLAN.md 4). The ring is a VecDeque allocated once to DecodeAheadOptions::capacity (default 8); a full ring parks the worker on a condvar instead of queueing, and seek_to() clears the ring in place and bumps a generation so in-flight frames from the old position are dropped rather than delivered. Occupancy, per-frame decode time, drops, seeks and backpressure stalls are exposed both as DecodeAheadStats and as tracing events on sub_media::decode_ahead. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-media (90 tests, including four fixture-gated decode-ahead tests and a tracing-capture test).
<!-- SECTION:FINAL_SUMMARY:END -->
