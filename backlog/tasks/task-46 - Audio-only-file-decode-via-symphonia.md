---
id: TASK-46
title: Audio-only file decode via symphonia
status: In Progress
assignee:
  - '@opus-task-46'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 03:04'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-13
references:
  - docs/PLAN.md
priority: high
ordinal: 67000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decision-4: audio-only files skip GStreamer.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 WAV, FLAC, MP3, AAC and Ogg decode to f32 PCM with seek support
- [x] #2 Probe reports duration and channels for audio-only items
- [x] #3 Test decodes the WAV and FLAC fixtures bit-exactly
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add symphonia 0.6 (features aac, isomp4, mp3 on top of defaults: wav, flac, ogg/vorbis, mkv, pcm) to sub-audio, with sub-core, sub-time, tracing and sub-test-support dev-dep.
2. New sub-audio module 'decode': AudioInfo (codec, container, sample rate, channel count, duration as RationalTime at the sample-rate timebase, frame count, seekable) and probe_audio(path) built on symphonia's format probe, so audio-only files never touch GStreamer (decision-4).
3. FileDecoder: open(path), info(), next_block() yielding interleaved f32 frames with an exact RationalTime start, seek(RationalTime) that is sample accurate (container seek in track timebase, decoder reset, then discard the residual frames), and decode_all() convenience returning owned interleaved f32 PCM. Gapless trim_start/trim_end honoured. Integer maths only, never floats for timing.
4. Stable audio.* error codes in sub-audio: file_unreadable, unsupported, decode_failed, seek_failed, no_audio_track.
5. Tests: unit tests for codes and for synthesised in-memory WAV; integration test over the generated fixtures decoding tone_48k_stereo.wav and .flac and asserting the two are bit-identical, plus seek round-trips; MP3/AAC/Ogg covered by unit round-trips only where a fixture exists, otherwise by the codec registry check.
6. Verify with cargo fmt --check, clippy -D warnings, cargo test -p sub-audio (fixtures via SUB_FIXTURES_DIR).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented sub-audio::decode on symphonia 0.6 (features aac, isomp4, mp3 on top of the defaults wav, flac, ogg/vorbis, mkv, pcm), so audio-only files never touch GStreamer (decision-4).

API: probe_audio(path) -> AudioInfo (container, codec, sample rate, channels, duration, seekable); FileDecoder::open/info/position/next_block/seek/decode_to_end; decode_file(path) -> Pcm. Blocks are interleaved f32 borrowed from a buffer the decoder reuses, so steady-state decoding does not allocate; nothing here runs in the audio callback. Time is integer only: every position is a RationalTime whose rate is the file's sample rate and whose value is a whole audio frame, and the container timebase conversions are exact u128 integer maths. seek() is sample accurate, not packet accurate: the demuxer seeks to the packet at or before the target, the decoder is reset, and the residual frames are decoded and dropped, so the next block starts exactly on the requested frame. Gapless trim_start/trim_end are honoured. New stable codes: audio.file_unreadable, audio.unsupported, audio.decode_failed, audio.seek_failed, audio.no_audio_track, audio.unsupported_layout.

Verification (all run in this environment):
- cargo fmt --all -- --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean (GStreamer sysroot sourced for sub-media).
- cargo test -p sub-audio with SUB_FIXTURES_DIR pointed at the generated fixtures: 11 unit + 4 integration + 1 doctest, all pass.
- AC #3: tests/decode_fixtures.rs decodes tone_48k_stereo.wav and tone_48k_stereo.flac and compares every interleaved sample by to_bits(); they are bit-identical, and seeks to frames 0, 1, 4001, 96000 and 239999 in the FLAC return blocks that are bit-identical to the same offsets of the whole-file decode.
- AC #2: probe_audio reports channels, sample rate and an exact frame-count duration; asserted against the manifest for every audio-only fixture and against synthesised WAV files in unit tests.

AC #1 is left unchecked: WAV and FLAC decode to f32 with sample-accurate seek is proven against real files, but this machine has no MP3, AAC or Ogg fixture and no encoder to make one (GStreamer is only present as a headers-and-core-elements sysroot, with no lamemp3enc, voaacenc or vorbisenc, and no ffmpeg). The MP3, AAC and Ogg Vorbis decoders are compiled in and a unit test asserts they are registered in the codec registry, and tests/decode_fixtures.rs decodes tone_48k_stereo.mp3/.m4a/.aac/.ogg automatically if a fixture set ever contains them, but the criterion is not proven here. Adding those fixtures to scripts/gen-fixtures.sh would be a scope change (and risks the existing sub-media probe duration assertions, since lossy encoders add delay and padding), so it was not done.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added sub-audio::decode: a symphonia-based probe and file decoder for audio-only media (decision-4), yielding interleaved f32 blocks with exact RationalTime positions in audio frames and sample-accurate seeking, with stable audio.* error codes. Verified by cargo fmt --check, workspace clippy with -D warnings, and cargo test -p sub-audio against the generated fixtures, where the WAV and FLAC tone fixtures decode bit-identically and seeks land on the exact frame. Left In Progress because acceptance criterion 1 cannot be proven here: MP3, AAC and Ogg decoders are compiled in and registry-tested, but this machine has no fixture or encoder for those formats.
<!-- SECTION:FINAL_SUMMARY:END -->
