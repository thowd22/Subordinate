---
id: TASK-47
title: Resampling to sequence sample rate with rubato
status: Done
assignee:
  - '@opus-task-47'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 16:39'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-45
  - TASK-46
references:
  - docs/PLAN.md
priority: high
ordinal: 68000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sources at 44.1 kHz and 48 kHz mix on one timeline.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Per-source resampler converts to the sequence rate with a fixed latency reported to the mixer
- [x] #2 Resampler runs off the audio thread and feeds a ring buffer
- [x] #3 Test: a 1 kHz tone at 44.1 kHz resampled to 48 kHz has the expected frequency and no clicks at chunk boundaries
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add rubato 5 (default-features off, sinc async only) and ringbuf 0.5 to sub-audio.
2. New module crates/sub-audio/src/resample.rs: per-source Resampler wrapping rubato Async sinc with a fixed input chunk, exact ratio from source rate to sequence rate, passthrough when the rates match; fixed latency reported as output_delay frames and as a RationalTime at the sequence rate.
3. Lock-free SPSC ring (ringbuf HeapRb) as PcmRing/PcmWriter/PcmReader: the reader side is the audio-callback side and neither locks nor allocates.
4. ResampleStage ties a Resampler to a PcmWriter, keeps a carry buffer so a full ring never drops frames, and is Send so it runs on a worker thread.
5. New stable error code audio.resample_failed; reuse audio.unsupported_layout and core invalid_argument for bad shapes.
6. Tests: latency is fixed and non-zero; 1 kHz tone 44.1k -> 48k has the right frequency (zero-crossing and Goertzel), amplitude, and no clicks at chunk boundaries with ragged input chunk sizes; worker-thread stage feeds a ring that a consumer drains without allocating.
7. Verify with cargo fmt --check, clippy -D warnings, cargo test -p sub-audio.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented crates/sub-audio/src/resample.rs.

Design:
- Resampler wraps rubato 5 Async sinc (256-tap BlackmanHarris2, cubic, 256x oversampling) with FixedAsync::Input and a 1024-frame input chunk, so the ratio and therefore the delay are fixed for the life of the source. latency_frames() is rubato's output_delay(); latency() returns it as an exact RationalTime at the sequence rate, which is what the mixer is told once (AC 1). Matching rates take a zero-latency pass-through with no filtering.
- Ragged input is buffered in a carry vector and only whole chunks are converted, so splitting a stream anywhere gives bit-identical output (test chunking_does_not_change_the_result). flush() pushes the partial chunk plus silent chunks and truncates to latency + frames_in * sequence_rate / source_rate computed in u128 integer arithmetic - no float timeline math.
- pcm_ring()/PcmWriter/PcmReader wrap ringbuf 0.5 HeapRb as a lock-free SPSC ring that only ever moves whole interleaved frames. PcmReader::read is a plain pop_slice: no lock, no allocation, safe for the audio callback. ResampleStage joins a Resampler to a PcmWriter and keeps a backlog so a full ring never drops frames; it is the worker-thread half (AC 2).
- New stable error code audio.resample_failed; audio.unsupported_layout and core invalid_argument cover bad shapes.

Verification (all run locally):
- cargo test -p sub-audio: 19 lib + 5 fixture + 2 doc tests pass.
- AC 3 test tone_keeps_its_frequency_and_has_no_clicks_at_chunk_boundaries: one second of 1 kHz at 44.1 kHz fed in cycling chunk sizes 17/1/4096/511/1024/3 frames, resampled to 48 kHz. Output length is exactly 48000 + latency frames; measured frequency from interpolated rising zero crossings is within 0.5 Hz of 1 kHz; peak stays in 0.98..1.02; and the second difference of every output sample stays under 4 sin^2(pi f / fs) * 1.05, the bound a clean 1 kHz sine cannot exceed - a click at any chunk boundary would break it.
- stage_runs_off_the_audio_thread_and_feeds_the_ring: a worker thread feeds the stage while the main thread drains the ring into a pre-allocated 256-frame block, and the collected tone still measures 1 kHz.
- cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (exit 0).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added a per-source sample-rate converter to sub-audio: Resampler (rubato 5 sinc, fixed input chunk, fixed latency reported to the mixer as an exact RationalTime at the sequence rate, pass-through when rates match), a lock-free SPSC PCM ring whose reader side neither locks nor allocates, and ResampleStage that runs on a worker thread and feeds that ring without dropping frames. New stable code audio.resample_failed. Verified with cargo test -p sub-audio (26 tests), including a 1 kHz 44.1 -> 48 kHz tone test that measures frequency within 0.5 Hz, checks amplitude and bounds the second difference to rule out clicks at chunk boundaries, plus a worker-thread-to-ring test; cargo fmt --all --check and cargo clippy --workspace --all-targets -- -D warnings both clean.
<!-- SECTION:FINAL_SUMMARY:END -->
