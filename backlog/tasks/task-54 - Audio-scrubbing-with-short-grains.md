---
id: TASK-54
title: Audio scrubbing with short grains
status: Done
assignee:
  - '@opus-task-54'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 14:08'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-50
references:
  - docs/PLAN.md
priority: medium
ordinal: 75000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Hearing audio while scrubbing is expected in an NLE (§5.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Dragging the playhead plays short windows of audio around the position at reduced gain
- [x] #2 Grain length and enable toggle are settings
- [x] #3 No clicks at grain boundaries (windowed)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New sub-audio module `scrub`: ScrubSettings (enable toggle, grain length as RationalTime, reduced gain in dB) with validation returning SubError with existing stable codes; a lock-free ScrubControl/ScrubPlayer pair sharing atomics only (no lock, no allocation on the audio thread).
2. Grain rendering: a request carries the playhead position; the player seeks the mixer transport to the grain start, renders grain_frames and applies a raised-cosine (Hann) envelope times the reduced gain, so every grain starts and ends at exactly zero (no clicks); between grains the output is silence and the transport stays put.
3. Mixer::with_scrub attaches a player; Mixer::process honours it, splitting blocks at grain boundaries. With no player, or the toggle off, the mixer behaves exactly as before.
4. UI: AudioSettingsPanel grows a 'Scrub audio' toggle and a grain-length slider, emitting an AudioSettingsAction; app.rs shares the ScrubControl through the mixer factory, applies the settings and asks for a grain whenever the playhead is dragged while not playing.
5. Tests: settings validation, grain windowing (zero at both boundaries, monotone rise, symmetric), reduced gain, silence between grains, transport seek per grain, toggle off is a no-op, and UI action tests. Verify fmt, clippy -D warnings and cargo test for sub-audio and sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

- New crate module `sub_audio::scrub` (crates/sub-audio/src/scrub.rs): `ScrubSettings` (enable toggle, grain length as an exact `RationalTime` in a milliseconds timebase, reduced gain in dB, default 60 ms at -9 dB), validated with existing stable codes (`INVALID_ARGUMENT` for a grain outside 5..=500 ms, `audio.invalid_gain` for a gain above unity or not finite, `audio.graph_invalid` for a negative scrub position). `scrub()` hands back a `ScrubControl` (engine thread) and a `ScrubPlayer` (audio callback) sharing nothing but relaxed atomics; a grain request is one packed u64 store (16-bit sequence number, 48-bit frame position) so the callback can never read a new request against an old position.
- `Mixer::with_scrub` attaches the player. While it is engaged the player owns the transport: it seeks the mixer to the grain start, the mixer renders there as usual, and the player multiplies the block by a raised-cosine (Hann) envelope times the scrub gain. Blocks are cut at grain boundaries so the shaping does not depend on the callback's block length. Between grains the mixer emits silence and the transport stays put. With the toggle off, or no player attached, `Mixer::process` behaves exactly as before. Nothing in the callback locks, allocates or frees.
- UI: the audio settings panel grew a 'Play audio while scrubbing' checkbox and a grain-length slider over the player's own 5..500 ms range, emitting `AudioSettingsAction::SetScrub`. `SubordinateApp` shares the scrub control through the mixer factory, applies (and reports refusals of) settings, asks for a grain whenever the playhead moves by hand while nothing is playing, and holds the output stream open for one grain length so a grain is not cut off by the stream closing.

Decisions

- A grain starts at the position the drag landed on and runs forward, rather than being centred on it: centring would sound timeline the user has not pointed at yet, and a forward grain is what an NLE scrub sounds like.
- Grain length is a `RationalTime` at a 1000-tick timebase and is converted to frames with the mixer's existing exact `frames_at`, so no float ever touches the length (§5.1).
- The wall-clock `Instant` in the app only decides how long to hold the device stream open; it is never used for timeline maths.

Validation

- `cargo fmt --all --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo test -p sub-audio` 114 unit tests + 8 doc tests pass, including new scrub unit tests (windowing to exact silence at both boundaries, symmetry, largest neighbouring step below 0.01, peak at the scrub gain and never above it, block-length independence, silence between grains, transport seek per grain, toggle disengages, settings applied or rejected whole) and new mixer tests (a grain of a real clip's ring plays windowed at reduced gain then falls silent, starts where the drag landed, and a disabled scrub leaves the mixer free-running at full gain).
- `cargo test -p sub-ui` all suites pass, including the new `tests/audio_scrub_settings.rs` egui_kittest interaction test that clicks the toggle by its accessibility label and applies what the panel asks for to a real `ScrubControl`.
- Not verifiable in this environment: hearing the grains on a real output device (no audio hardware here), and the mix itself is still silence until the sequence's clips are fed to the mixer graph (that wiring is a later task); the scrub path is verified against the mixer with clip rings installed instead.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added sub_audio::scrub: dragging the playhead now plays a short windowed grain of the mix from where the drag landed, at a reduced gain, driven by a lock-free ScrubControl/ScrubPlayer pair whose audio half neither locks nor allocates. The grain length and the enable toggle are ScrubSettings, exposed in the audio settings panel and applied to the player by the app, and every grain is multiplied by a raised-cosine envelope that is exactly zero on its first and last frame, so no boundary clicks. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test for sub-audio (unit, doc) and sub-ui (including a new egui_kittest interaction test driving the toggle).
<!-- SECTION:FINAL_SUMMARY:END -->
