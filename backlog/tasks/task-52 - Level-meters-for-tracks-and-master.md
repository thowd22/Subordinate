---
id: TASK-52
title: Level meters for tracks and master
status: Done
assignee:
  - '@opus-task-52'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 20:24'
labels:
  - ui
  - audio
milestone: m-3
dependencies:
  - TASK-49
references:
  - docs/PLAN.md
priority: medium
ordinal: 73000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users need to see levels; meters must be read without locking the audio thread.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Peak and RMS per track and master published via atomics from the callback
- [x] #2 Meter widget with peak hold and clip indicator in the timeline headers and a master meter in the viewer
- [x] #3 Meter update costs under 0.1 ms per frame
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-audio: new meter module with MeterBank (per-track + master AtomicU32 peak/RMS cells, relaxed stores from the callback), MeterLevels, and a levels_of() block measurement.
2. sub-audio/mixer: render each track through a preallocated track buffer so a post-fader track level exists, publish per-track levels and the post-master-gain master level from Mixer::render; Mixer::with_meters attaches the bank. No lock, no allocation in the callback.
3. sub-ui: new meter module with MeterState (peak hold + decay, latched clip indicator) and a painter that draws a dB-scaled bar, peak-hold line and clip square.
4. Wire it: track meters in the timeline track headers (HeaderLayout gains a meter rect, TrackHeaderState holds per-track MeterState), master meter in the viewer transport row, and app.rs shares the MeterBank with the mixer factory and pumps it each frame.
5. Tests: meter maths, mixer publication (silence, unity, muted, master gain), UI hold/decay/clip latch, layout, and a timing test for the per-frame meter update cost.
6. Verify with cargo fmt, clippy -D warnings and the touched crates' tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

sub-audio: new `meter` module. `MeterBank` holds one `MeterCell` per track plus one for the master; a cell is two `AtomicU32`s carrying the f32 bit patterns of peak and RMS, so the callback publishes with relaxed stores only — no lock, no allocation, no reader it has to wait for. `levels_of` measures an interleaved block in one pass (peak as the largest magnitude, RMS summed in f64 across all channels).

sub-audio/mixer: `Mixer::render` now sums each track into a preallocated lane buffer (sized once, alongside the existing clip scratch), measures that lane post-fader, adds it into the output, and measures the master after the master fader. A muted or soloed-out track still drains its rings and meters silent. `Mixer::with_meters` attaches an `Arc<MeterBank>` on the engine thread; a graph with more tracks than the bank has cells simply leaves them unmetered, and cells past the graph's track count are published silent so a removed track's meter does not stick.

sub-ui: new `meter` module. `MeterState` is four floats — the latest levels, the held peak in dB, the hold timer and the clip-latch timer — updated once per frame with the seconds elapsed. Peak hold sits for 1.5 s then falls at 24 dB/s and never below the level being shown; a clipped block (peak >= 1.0) latches the indicator for 2 s. The bar is dB-scaled from -60 dB, painted green, amber above -6 dB, red when clipping.

Wiring: `HeaderLayout` gained a `meter` rect in the control row between the kind label and the mute/lock toggles; `TrackHeaderState` keeps one `MeterState` per `TrackId` (`update_meter`, `decay_meters`, `retain_meters`) and paints it each frame. `TimelinePanel::header_state_mut` exposes it so the engine can feed it. `ViewerPanel` gained a `master_meter` drawn at the right of the transport row plus `update_master_meter`; `SubordinateApp` owns the shared `MeterBank` (64 track cells), hands it to every mixer the output stage builds, and pumps the master meter from `stable_dt` each frame.

Verification

- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-audio -p sub-ui: all pass (sub-audio 90 lib tests, sub-ui 185 lib + integration suites).
- AC 1: `mixer::tests::track_and_master_levels_are_published_every_block` proves per-track and post-master-fader levels reach the bank through the atomics; companion tests cover a muted track, a bank too small for the graph, a track leaving the graph, and clipping.
- AC 2: `tests/track_headers.rs::a_track_header_paints_the_level_meter_it_was_fed_and_lights_it_on_a_clip` drives a real headless egui frame and finds the clip-coloured rectangle inside the first header's meter rect and nothing in the second's; `viewer::tests::the_painted_viewer_draws_a_master_meter_that_lights_when_the_master_clips` does the same for the master meter in the viewer. The timeline panel snapshot was regenerated for the new header meters.
- AC 3: `meter::tests::updating_a_full_set_of_meters_costs_well_under_a_tenth_of_a_millisecond` reads 65 cells out of a real `MeterBank` and folds them into 65 `MeterState`s — more meters than a session has — and asserts the whole per-frame update stays under 100 us. Measured here: 0.8 us per frame.

Notes

- The mixer's summation order changed (clips into a track lane, lanes into the master) so that a post-fader track level exists at all; the existing mixer tests, including the gain-multiplication and stereo-sum ones, still pass unchanged.
- Track meters are indexed by the track's position in the mix graph. Feeding them from a live sequence is the engine's job and lands with the playback engine task; the header column exposes `update_meter` for it and paints a silent meter until then.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added level metering end to end: a lock-free MeterBank of atomic peak/RMS cells in sub-audio that the mixer publishes into per track (post-fader) and for the master (post master fader) from every rendered block, and a MeterState widget in sub-ui with dB scaling, peak hold and a latched clip indicator, drawn in every timeline track header and as a master meter in the viewer transport row, with the app sharing one bank with the output stage. Verified with cargo fmt --all --check and cargo clippy --workspace --all-targets -D warnings (both clean) and cargo test -p sub-audio -p sub-ui: mixer tests prove the published levels for playing, muted, oversized-graph and clipping cases, headless egui frames prove the header and viewer meters paint and light on a clip, and a timing test measures a 65-meter per-frame update at 0.8 us against the 100 us budget.
<!-- SECTION:FINAL_SUMMARY:END -->
