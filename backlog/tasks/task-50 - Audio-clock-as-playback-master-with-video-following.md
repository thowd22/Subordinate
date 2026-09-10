---
id: TASK-50
title: Audio clock as playback master with video following
status: Done
assignee:
  - '@opus-task-50'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 10:29'
labels:
  - audio
  - media
milestone: m-3
dependencies:
  - TASK-49
  - TASK-23
references:
  - docs/PLAN.md
priority: high
ordinal: 71000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sync is defined by audio; video frames are chosen from the audio position (§5.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Playhead time derives from samples rendered by the callback plus output latency
- [x] #2 Video scheduler picks the frame for the audio-derived time; the video-only clock is removed
- [x] #3 Drift test over 10 minutes of the long fixture stays under one frame
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-audio: add an AudioClock (module clock.rs) holding relaxed atomics the callback stores into — sequence frames rendered by the mixer plus the output latency of the block just written — so the audible playhead is rendered-frames minus latency, published as an exact RationalTime at the sequence sample rate. Wire it through OutputRenderer, OpenStream, OutputStream and AudioOutput. No locks or allocation added to the callback.
2. sub-edit/playback.rs: remove the scheduler's own video clock. Split the wall-clock accumulator out into MonotonicClock, the fallback master used when no audio stream is running, and give PlaybackScheduler::follow(master: RationalTime) which picks the video frame containing the master time (floor at the sequence timebase), reporting frames advanced, dropped presentations, loop wrap and stop. advance(elapsed) now just runs the fallback master through follow.
3. sub-ui: keep the MixerControl half alive, start/stop the output stream with the transport, seek the mixer transport with the playhead, and follow the audio clock when the stream is producing; fall back to the monotonic master otherwise.
4. Tests: unit tests for the clock arithmetic and for follow (frame choice, drop counting, loop wrap, stop), plus a ten-minute drift test driving real callback-sized blocks through the renderer into the scheduler at 23.976 fps and asserting the chosen frame never differs from the audio-derived frame by more than one.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Audio is now the playback master.

sub-audio: new `clock` module. `AudioClock` holds three relaxed atomics the output callback stores into once per block — the mixer transport position after the block (sequence frames), the frames of it still sitting in the device buffer, and whether anything has been published since the last reset. The audible playhead is rendered minus latency, returned as an exact `RationalTime` at the sequence sample rate and `None` until the callback has produced a block, so a stale reading cannot yank the playhead. `OutputRenderer` builds one, publishes into it from `render`/`render_i16`/`render_u16` (three relaxed stores, one integer rate conversion; no lock, no allocation added to the callback), and it is exposed through `OpenStream`, `OutputStream::clock` and `AudioOutput::clock`.

sub-edit: the video-only clock is gone from `PlaybackScheduler`. Its wall-clock accumulator moved out into `MonotonicClock`, the fallback master used when no stream is playing, and the scheduler now only chooses a frame for a master time: `follow(master)` floors the master into the sequence timebase, applies the loop range or the end of the sequence, and counts the presentations the master ran past as dropped. `advance(elapsed)` is the fallback master plus `follow`, so the engine and every existing caller keep working. `resync_master()` drops the baseline when the master jumps for a reason that is not playback (stream reopened, clock reset by a seek).

sub-ui: the app keeps the `MixerControl` half its mixer factory builds, opens the output stream when playback runs at 1x and closes it at any other speed or on pause, seeks the audio transport with the playhead and on a loop wrap, and follows the audio clock's position when the stream is producing — otherwise the monotonic master. A device that will not open is logged and playback carries on off the fallback.

Latency: measured as the block the callback has just written, converted to sequence frames with integer arithmetic. That is the buffer in flight; any further device-side latency the host reports is not modelled here.

AC3 caveat: `crates/sub-audio/tests/av_drift.rs` runs ten minutes of real callbacks (48 kHz sequence rendered to a 44.1 kHz device, so the resampling path is exercised) against the long fixture's declared timeline — ten minutes at 25 fps, read from fixtures/manifest.json — and asserts after every block that the frame on screen is the frame the audible sample count falls inside, i.e. drift stays under one frame, plus that the transport has rendered the samples those device frames are worth to within two frames. It does not decode the fixture: longgop_720p_10min.mp4 is video-only, so there is no audio track to play, and driving decoded media through the same path is TASK-56's harness.

Verification (Linux, no audio device, GStreamer from the local prefix): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-audio -p sub-edit -p sub-ui all green, including the new clock unit tests, nine new scheduler/master tests, tests/av_drift.rs and the two egui_kittest interaction tests in crates/sub-ui/tests/transport_audio_master.rs.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Sync is now defined by audio: the output callback publishes an AudioClock (samples rendered by the mixer, less the block still in the device buffer) and PlaybackScheduler no longer runs a clock of its own — it picks the frame covering whatever master time it is given, with a MonotonicClock fallback for when no stream is playing. The app follows the audio clock while the stream runs at 1x and seeks the audio transport with the playhead. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings, and cargo test across sub-audio, sub-edit and sub-ui, including a ten-minute drift test over real resampling callbacks that keeps video inside the audio frame throughout.
<!-- SECTION:FINAL_SUMMARY:END -->
