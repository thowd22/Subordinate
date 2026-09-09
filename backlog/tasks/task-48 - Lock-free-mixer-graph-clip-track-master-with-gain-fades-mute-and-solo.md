---
id: TASK-48
title: 'Lock-free mixer graph: clip, track, master with gain, fades, mute and solo'
status: Done
assignee:
  - '@opus-task-48'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 18:14'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-47
  - TASK-12
references:
  - docs/PLAN.md
priority: high
ordinal: 69000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The real-time audio callback must never lock or allocate (§4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Mixer pulls from per-clip ring buffers, applies clip gain and fades, track gain, mute and solo, sums to master
- [x] #2 Graph updates from the engine are applied via atomic swap of an immutable graph description
- [x] #3 A test wraps the callback with an allocation-detecting allocator and asserts zero allocations over 10 seconds
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New module crates/sub-audio/src/mixer.rs holding an immutable graph description (MixGraph: master gain/mute, tracks with gain/mute/solo, clip nodes with slot index, start/duration/fades as exact frame counts derived from RationalTime, linear gains) built off the audio thread by MixGraphBuilder, which validates shapes and returns SubError with new stable audio.* codes.
2. Callback side: Mixer owns preallocated clip slots (PcmReader per clip ring), a preallocated scratch block and the current Arc<MixGraph>. Mixer::process fills an interleaved output block: per track, skip muted/non-solo lanes, pull each active clip from its ring, apply clip gain and linear fade in/out ramps, sum through track gain, then master gain and mute. Underruns become silence; nothing locks or allocates.
3. Graph and slot updates cross the thread boundary through a lock-free SPSC ring of MixerUpdate values (ringbuf, already a dependency): the engine publishes, the callback swaps the new value in atomically and pushes the displaced one back over a retire ring so no Arc or reader is ever dropped on the audio thread. MixerControl is the engine-side handle.
4. Unit tests in the module for gain, fades, mute, solo, master, underrun and the swap handover; integration test crates/sub-audio/tests/mixer_no_alloc.rs installs a counting global allocator, arms it around 10 seconds of 48 kHz callbacks and asserts zero allocations.
5. Verify with cargo fmt --check, clippy pedantic -D warnings and cargo test -p sub-audio (no GStreamer needed).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented crates/sub-audio/src/mixer.rs plus crates/sub-audio/tests/mixer_no_alloc.rs.

Design: MixGraphBuilder validates and compiles an immutable MixGraph on the engine thread. Every timeline value is taken as RationalTime and turned into whole audio frames there (exact where the rates divide, nearest-rounded for 1001-denominator rates), so the callback only ever does integer and f32 arithmetic. Gains are given in decibels over the same range as sub_model::GainDb (-144..=+24) and converted to linear factors at build time; -144 dB is silence. Solo is resolved at build time into a per-track 'audible' flag.

Graph handover: rather than arc-swap (a new dependency, and its load guard can leave the audio thread holding the last Arc reference), the two halves share a lock-free SPSC ring of MixerUpdate values. The callback takes the published Arc<MixGraph>, PcmReader or seek position, swaps it in with mem::replace and pushes whatever it displaced onto a second ring for MixerControl::collect_retired to free. An update is only taken when there is room to hand its predecessor back, so nothing is ever dropped or freed on the audio thread. Mixer::process itself only fills, reads rings, multiplies and adds: slots, scratch and both rings are preallocated in mixer().

A muted or soloed-out track still drains its clips' rings so it stays aligned with the transport; a clip with no ring, or a ring that has run dry, contributes silence and bumps Mixer::underrun_frames.

New stable error codes: audio.invalid_gain and audio.graph_invalid (added to the codes module and its uniqueness test).

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (GStreamer env sourced for sub-media); cargo test -p sub-audio green - 36 unit tests (17 new in mixer::tests covering gain multiplication, linear fade ramps, clip placement, mute, solo, master mute, track summing, underruns, multi-pass blocks, graph swap and retirement, slot clearing, seek, RationalTime-to-frame conversion and every rejection path), 5 fixture tests, 3 doctests and tests/mixer_no_alloc.rs.

AC3 evidence: mixer_no_alloc.rs installs a counting #[global_allocator] armed only around Mixer::process, renders 937 blocks of 512 frames at 48 kHz (ten seconds of audio, not wall-clock) through a three-track stereo graph while publishing new graphs, reinstalling a slot and seeking between blocks, and asserts zero allocations, reallocations and frees, with zero underruns.

Note for TASK-51: track gain and solo have no home in the project model yet (Track carries only muted/locked), so the mixer takes them as graph inputs; wiring them to commands is that task's work.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the lock-free mixer graph in crates/sub-audio/src/mixer.rs: an immutable MixGraph compiled on the engine thread from RationalTime spans into whole audio frames, and a Mixer whose callback pulls each clip from its own PCM ring, applies clip gain and linear fades, track gain with mute and solo, and sums into a master bus with its own gain and mute. Graph, slot and seek changes cross to the callback over a lock-free SPSC ring and whatever they displace is handed back over a retire ring, so the audio thread never locks, allocates, drops or frees. Verified with cargo fmt --check, workspace clippy at -D warnings, 17 new unit tests and tests/mixer_no_alloc.rs, which counts allocations with a global allocator armed around the callback over ten seconds of 48 kHz audio and sees zero.
<!-- SECTION:FINAL_SUMMARY:END -->
