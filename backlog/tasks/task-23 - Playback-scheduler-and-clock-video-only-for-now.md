---
id: TASK-23
title: Playback scheduler and clock (video-only for now)
status: Done
assignee:
  - '@opus-task-23'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 08:43'
labels:
  - media
  - ui
milestone: m-1
dependencies:
  - TASK-22
references:
  - docs/PLAN.md
priority: high
ordinal: 44000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Play/pause needs a clock that advances the playhead and requests frames ahead; the audio clock replaces it in phase 3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Play, pause, JKL shuttle at 1x/2x/4x forward and reverse, loop range
- [x] #2 Scheduler drops frames rather than stalling when decode falls behind and logs drop counts
- [x] #3 Playhead position is published as an engine event
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub-edit::playback: a float-free PlaybackScheduler (exact rational clock, residue accumulator in integer units), ShuttleSpeed for JKL 1x/2x/4x forward and reverse, pause/toggle, seek, optional loop range, and a Tick that reports frames advanced, wrap and dropped presentations.
2. Frame dropping: the clock follows wall time, so a tick that spans several frame intervals skips the missed presentations, counts them and logs the running drop count with tracing (never stalls waiting for decode).
3. Generalise EventBus/EventReceiver over the event type (defaulting to ChangeEvent) so the engine can carry a second bus.
4. Engine: own the scheduler on the engine thread, serve transport requests (play forward/backward, pause, seek, loop range, timebase, status) and tick with recv_timeout while playing, publishing PlayheadEvent on the playhead bus; EngineHandle gains subscribe_playhead and the transport methods.
5. Wire the UI: app.rs maps the transport actions (J/K/L/Space) to a scheduler and advances it from the frame delta, moving the viewer playhead and requesting repaints while playing.
6. Tests: unit tests for the clock, shuttle cycling, loop wrap, end-of-sequence stop and drop counting; engine tests for transport requests and playhead events; UI test for the transport keys.
7. Verify with cargo fmt, clippy -D warnings and cargo test for sub-edit and sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

- New module sub-edit/src/playback.rs holds PlaybackScheduler, the video-master clock. It is driven by wall-clock Durations handed to advance(), so it is deterministic and testable without a timer. All arithmetic is integer: elapsed nanoseconds are accumulated in units scaled by the timebase numerator, so a frame boundary falls at exactly the same instant every run and 24000/1001 does not drift (one second at 23.976 is 239 whole frames in the test). No floats anywhere in the clock.
- ShuttleSpeed models JKL: L steps 1x/2x/4x forward, J the same in reverse, K pauses, space toggles; turning round from forward goes to 1x reverse rather than jumping speed. Reverse and forward both honour the loop range (rem_euclid wrap) and, without one, stop at frame zero or the last frame.
- Frame dropping: the position follows the wall clock, not the last frame presented, so a tick spanning N frame intervals counts N-1 missed presentations, adds them to the run's drop count and logs them with tracing::warn (dropped, total, speed). Playback never waits for a late decode. The count is per run of playback and resets when play starts again.
- The playhead is not project state, so it is not a Command and not undoable (this matches the note already in sub-ui/src/viewer.rs). It is engine state: the engine thread owns the scheduler, serves PlaybackOp requests (play forward/backward, pause, toggle, set speed, seek, loop range, timebase, follow sequence, status) and, while playing, waits on recv_timeout(time_until_next_frame) so it ticks on time without polling.
- EventBus/EventReceiver are now generic over the event type with ChangeEvent as the default, so the engine runs a second bus for PlayheadEvent without a second implementation and no existing caller changed. EngineHandle::subscribe_playhead() is the new stream; editing subscribers never see playhead traffic.
- UI: viewer.rs gains TransportAction (the pure Action -> transport mapping, mirroring ViewerAction) and app.rs owns a PlaybackScheduler wired to J/K/L/space. run_transport advances it by the frame delta, moves the viewer playhead, and requests a repaint while playing; a scrub or frame step while playing is fed back into the clock so playback carries on from there. egui's float frame delta is converted to whole nanoseconds at that boundary and nowhere else.
- Errors: an empty or negative loop range is edit.invalid_time, following an unknown sequence is edit.sequence_not_found. No new error codes were needed.

Verification: cargo fmt --all --check clean; cargo clippy -p sub-edit -p sub-ui --all-targets -- -D warnings clean; cargo test -p sub-edit (82 unit + doctests, including 18 playback and 4 new engine transport tests) and cargo test -p sub-ui --lib (191 tests) all pass. sub-ui needs the local GStreamer env (env-gst.sh) to build here.

Verification (final): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (exit 0); cargo test -p sub-edit, -p sub-ui --lib and -p sub-command all pass.

Evidence per criterion:
- AC1: playback::tests::jkl_shuttles_through_the_speeds, shuttle_speed_scales_the_frames_covered_per_tick, reverse_playback_runs_backwards_and_stops_at_zero, playback_stops_at_the_last_frame_without_a_loop, a_loop_range_wraps_instead_of_stopping, an_empty_loop_range_is_refused; engine::tests::the_transport_shuttles_and_reports_where_it_is and playing_off_the_end_stops_and_looping_wraps drive the same through the engine thread; viewer::tests::the_transport_keys_drive_the_playback_clock proves J/K/L/space reach the clock.
- AC2: playback::tests::a_late_tick_drops_frames_rather_than_falling_behind (a five-interval tick advances five frames and reports four drops) and the_drop_count_belongs_to_one_run_of_playback; the count is logged by tracing::warn! in PlaybackScheduler::advance with the running total.
- AC3: engine::tests::the_playhead_is_published_as_an_engine_event subscribes with EngineHandle::subscribe_playhead, sees the playhead published on transport changes and again as the engine thread's clock advances on its own, and asserts the ChangeEvent stream stays silent.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the video-master playback clock: sub-edit::playback::PlaybackScheduler advances the playhead from wall-clock deltas with exact integer arithmetic (no floats), supports play, pause, space toggle and JKL shuttling at 1x/2x/4x forward and reverse, and wraps a loop range instead of stopping at the ends. It follows the wall clock rather than the last frame shown, so a late tick skips the presentations it missed, counts them and logs the running total with tracing rather than stalling playback. The engine thread now owns the scheduler, serves transport requests (play/pause/toggle/speed/seek/loop range/timebase/follow sequence/status) and waits on recv_timeout so it ticks on time; the playhead is broadcast as a PlayheadEvent on a second event bus (EventBus is now generic with ChangeEvent as its default) reachable through EngineHandle::subscribe_playhead, and stays out of the ChangeEvent stream because the playhead is not project state and not undoable. The UI wires J/K/L/space to a scheduler in app.rs and moves the viewer playhead from it each frame. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings (both clean) and cargo test for sub-edit, sub-ui and sub-command; 18 new playback tests, 4 new engine transport tests and one UI transport-key test.
<!-- SECTION:FINAL_SUMMARY:END -->
