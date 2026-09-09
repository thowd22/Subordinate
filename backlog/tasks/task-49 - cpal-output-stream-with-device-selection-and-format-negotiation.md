---
id: TASK-49
title: cpal output stream with device selection and format negotiation
status: Done
assignee:
  - '@opus-task-49'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 19:21'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-48
references:
  - docs/PLAN.md
priority: high
ordinal: 70000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Audio must play on ALSA/PipeWire, WASAPI and CoreAudio.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Output opens the default device at the sequence rate or the closest supported rate with resampling
- [x] #2 Settings panel lists devices; switching reopens the stream without crashing
- [x] #3 Underruns are counted and surfaced in diagnostics
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add cpal 0.18 to sub-audio; add libasound2-dev to the Linux CI apt list (cpal's ALSA backend needs it).
2. New module sub-audio/src/output.rs:
   - OutputDevice enumeration (id, name, default flag, supported rate ranges/channels/sample formats).
   - Pure, hardware-free format negotiation: exact sequence rate when the device supports it, otherwise the closest supported rate flagged for resampling; nearest channel count; f32/i16/u16 sample format choice.
   - RateAdapter: allocation-free, drift-free output-rate conversion driven by an exact integer accumulator (num/den from the two rates, never a float clock), plus a channel map, feeding from Mixer::process into a preallocated scratch.
   - OutputMetrics: atomics for underrun frames, frames rendered, callbacks and stream errors; OutputDiagnostics snapshot for the diagnostics surface.
   - OutputBackend trait with CpalBackend (real) so device listing, opening, switching and fallback are testable headlessly with a stub backend.
   - OutputStream/AudioOutput handle: open(device), switch device by closing and reopening, restoring the previous device when the new one fails.
3. New stable error codes: audio.no_output_device, audio.device_unavailable, audio.format_unsupported, audio.stream_failed.
4. sub-ui: audio_settings.rs panel listing devices with a selection and an underrun/diagnostics readout; wire into the app menu next to the hardware diagnostics panel.
5. Tests: negotiation table tests, rate adapter accuracy/no-drift and no-allocation test, channel map tests, stub-backend device switching and fallback, panel state tests. Real device open is skipped where no audio hardware exists (WSL).
6. Verify with cargo fmt --check, clippy -D warnings, and cargo test for sub-audio and sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-audio/src/output.rs (new module, re-exported from the crate root) plus crates/sub-ui/src/audio_settings.rs and its wiring in crates/sub-ui/src/app.rs.

Design:
- cpal 0.18 (the version docs/PLAN.md picks) added to sub-audio; libasound2-dev added to the Linux apt lists in ci.yml and gpu-smoke.yml because cpal's ALSA backend needs it.
- The host is behind an OutputBackend trait (devices + open) so device listing, opening, switching and failure handling are testable without a sound card; CpalBackend is the real one and a stub backend covers the switching tests.
- negotiate() is a pure function over the shapes a device advertises: the sequence rate wins whenever a shape covers it, otherwise the closest reachable rate is taken and the stream is flagged for resampling; ties break on exact channel count, then a wider device over a narrower one, then f32 over i16 over u16.
- OutputRenderer is the callback body. It allocates every buffer at open time and converts inside the callback with no lock, no allocation and no syscall. The sample-rate conversion is linear interpolation on an exact integer clock (position held as phase/device_rate, advanced by source_rate per device frame), so it never accumulates the drift a float phase would; frames a pass mixes but does not reach are held for the next pass, so no source frame is skipped or read twice. A channel map handles mono fan-out, silent extra device channels and a mono downmix.
- OutputMetrics is atomics only on the callback path (three relaxed stores per block, including the mixer's cumulative underrun frames); the host error callback, which is not the audio thread, records stream errors and the last message. OutputDiagnostics is the snapshot the panel and the CLI print.
- AudioOutput owns the stream and the user's device choice. select_device closes and reopens with a fresh mixer from a factory (a reopened stream needs a mixer paired with a fresh MixerControl); when the new device will not open the previous one is restored and the error returned, so a bad choice never takes the editor down.
- New stable codes: audio.no_output_device, audio.device_unavailable, audio.format_unsupported, audio.stream_failed.
- The settings panel holds no audio state: it is given the device list and the diagnostics and returns an AudioSettingsAction the app applies to AudioOutput. That seam is what makes the panel testable headlessly and the switching testable without a window.

Environment caveat: this machine (WSL) has no sound card (/dev/snd holds only 'timer'), and hosted CI runners have none either, so cpal's build_output_stream itself is never executed by an automated test. Everything around it is: crates/sub-audio/tests/cpal_devices.rs asks the real platform host for its devices and negotiates a 48 kHz stereo format against each one (tolerating a host with no devices), and the stub backend covers open, switch, restore-on-failure and stop. Playing audible sound through a speaker remains a manual hardware check.

Validation (from the worktree, CARGO_BUILD_JOBS=2, GStreamer env sourced for the crates that need it):
- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-audio -p sub-ui: all green. sub-audio 62 lib tests (30 new in output::tests: negotiation table, channel map, passthrough, constant-signal and exact-ratio resampling both directions, multi-pass blocks, underrun counting, i16/u16 scaling, mismatched-format rejection, device listing order, switching, restore-on-failure, closed-panel selection, diagnostics), output_no_alloc.rs (48 kHz stereo -> 44.1 kHz 5.1 i16, ten seconds, zero allocations and zero frees while armed), cpal_devices.rs (real host). sub-ui: 6 new audio_settings tests.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the cpal output stage. sub-audio gains an output module: device enumeration, pure format negotiation (the sequence rate when the device covers it, otherwise the closest rate with resampling), an allocation-free callback renderer that converts rate and channel layout on an exact integer clock, atomic underrun and stream-error counters with an OutputDiagnostics snapshot, an OutputBackend seam with the real CpalBackend behind it, and an AudioOutput handle that reopens the stream on a device switch and restores the previous device when the new one will not open. sub-ui gains an audio settings panel that lists devices, shows the stream's state and underruns, and hands its selection to AudioOutput; libasound2-dev was added to the Linux CI apt lists. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test for sub-audio and sub-ui, including a no-allocation test over ten seconds of 48 kHz stereo resampled to a 44.1 kHz 5.1 16-bit device and a test that queries the real platform host. cpal's own build_output_stream is the one line no automated test reaches, because neither this machine nor a hosted runner has a sound card; audible playback stays a manual hardware check.
<!-- SECTION:FINAL_SUMMARY:END -->
