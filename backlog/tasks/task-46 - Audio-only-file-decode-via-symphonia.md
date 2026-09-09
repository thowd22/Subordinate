---
id: TASK-46
title: Audio-only file decode via symphonia
status: Done
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 15:51'
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
- [x] #1 WAV, FLAC, MP3, AAC and Ogg decode to f32 PCM with seek support
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

Requeued 2026-09-09 by supervisor: implementation merged on main, remaining criterion needs MP3, AAC and Ogg test media. The stable GStreamer env (source /home/admin2/.cache/subordinate/env-gst.sh) has gst-launch-1.0 with lamemp3enc, vorbisenc and avenc_aac available; extend scripts/gen-fixtures.sh (and the PowerShell mirror) to emit small tone files in those formats and test against them. CI generates fixtures on all three OSes.

Follow-up wave 3: added the missing MP3, AAC and Ogg test media, so acceptance criterion 1 is now proven against real files.

- scripts/gen-fixtures.sh and the PowerShell mirror now emit tone_48k_stereo.mp3 (lamemp3enc, 192 kbit/s CBR), tone_48k_stereo.m4a (avenc_aac + aacparse + mp4mux) and tone_48k_stereo.ogg (vorbisenc + oggmux) alongside the WAV and FLAC tones. The catalogue gained a `lossy` column and the manifest a `lossy` field (serde default, so manifests written before this still parse and the schema stays version 1). A lossy fixture whose encoder is not installed is skipped and recorded as `"generated": false` rather than failing the run, so a minimal GStreamer install still produces a usable fixture set.
- Duration expectations stay exact for lossless files and get a documented tolerance for lossy ones (100 ms in sub-media's probe test, 4800 frames in sub-audio's), because a lossy encoder brackets the signal with priming and padding frames that no container trims here.
- New sub-audio tests, driven by the manifest rather than a hard-coded list: every generated lossy fixture decodes to 48 kHz stereo f32 whose RMS is within 10% of the WAV reference, and seeking each of them to frames 0, 4001, 96000 and 200000 lands exactly on the requested frame and returns audio, not silence. The lossless WAV/FLAC bit-exactness and sample-accurate seek tests are unchanged.

Verification in this environment (fixtures generated with the stable GStreamer sysroot into a scratch directory, SUB_FIXTURES_DIR pointed at it):
- scripts/gen-fixtures.sh wrote tone_48k_stereo.mp3, .m4a and .ogg and a manifest carrying lossy: true for all three; symphonia decodes them to 241920, 241664 and 240000 frames against the authored 240000.
- cargo test -p sub-audio -p sub-test-support: 11 unit + 5 integration + 1 generated-fixture + 7 sub-test-support tests, all pass, including the two new lossy tests.
- cargo test -p sub-media --test probe_fixtures: 7 pass; the GStreamer probe reports duration, channels and rate for the three new audio-only files within the lossy tolerance.
- cargo fmt --all -- --check and cargo clippy --workspace --all-targets -- -D warnings: clean.

Not verifiable here: scripts/gen-fixtures.ps1 (no PowerShell on this machine); it was changed to mirror the bash script, and CI runs the bash script on all three OSes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added sub-audio::decode: a symphonia-based probe and file decoder for audio-only media (decision-4), yielding interleaved f32 blocks with exact RationalTime positions in audio frames and sample-accurate seeking, with stable audio.* error codes. The fixture generator now also produces MP3, AAC (in MP4) and Ogg Vorbis tones next to the WAV and FLAC ones, with a `lossy` flag in the manifest so tests hold lossless files exact and give lossy files a documented tolerance; a missing encoder skips its fixture instead of failing the run. Verified with cargo fmt --check, workspace clippy at -D warnings, cargo test -p sub-audio -p sub-test-support and sub-media's probe_fixtures against a freshly generated fixture set: WAV and FLAC decode bit-identically and seek to the exact frame, and every lossy fixture decodes to 48 kHz stereo at the reference signal level and seeks to the requested frame.
<!-- SECTION:FINAL_SUMMARY:END -->
