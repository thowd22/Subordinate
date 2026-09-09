---
id: TASK-45
title: Audio decode from video files via GStreamer to PCM
status: Done
assignee:
  - '@opus-task-45'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 04:36'
labels:
  - audio
milestone: m-3
dependencies:
  - TASK-14
references:
  - docs/PLAN.md
priority: high
ordinal: 66000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decision-4: video files are demuxed once, so their audio comes from the GStreamer pipeline.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Decoder exposes an audio appsink producing interleaved f32 PCM with sample-accurate PTS
- [x] #2 Channel layout and sample rate are reported; mono and 5.1 sources downmix to stereo with documented gains
- [x] #3 Test decodes fixture audio and checks sample count against duration
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-media/src/audio.rs: AudioFormat (sample rate, source channels, ChannelLayout, delivered channels), AudioBlock borrowing an interleaved f32 buffer with a sample-accurate RationalTime start at the source sample rate, and a pure downmix matrix with documented gains (mono -> both at unity; 5.1 BS.775: C and surrounds at -3 dB, LFE dropped; unrecognised layouts keep their first two channels).
2. Extend Decoder in decode.rs so one uridecodebin demuxes the file once (decision-4): DecoderOptions gains a StreamSelection (Video, VideoAndAudio, AudioOnly) and an audio branch audioconvert -> appsink with F32LE interleaved caps at the source rate/channels. Decoder::next_audio_block() pulls from it; format() reports the negotiated audio shape, waited for at open so it is always known.
3. New stable code media.no_audio_stream for a file opened for audio that carries none; reuse media.decode_failed/decode_timeout otherwise.
4. Add an A/V fixture (bars_720p_h264_flac.mkv, 5 s, 48 kHz stereo FLAC) to scripts/gen-fixtures.sh and gen-fixtures.ps1, since no existing fixture carries audio.
5. Tests: unit tests for the downmix gains and PTS arithmetic; integration test decoding the A/V fixture and checking the total sample count against the fixture duration exactly; integration tests that synthesise mono and 5.1 files with GStreamer and check the reported layout plus the stereo downmix.
6. Verify with cargo fmt --check, clippy -D warnings and cargo test for sub-media.

7. Revision to step 4/5: scripts/gen-fixtures.sh and .ps1 were left untouched. No manifest fixture carries audio, and adding one to the shared generator would have been a scope change (the same call TASK-46 made), so crates/sub-media/tests/audio_decode.rs synthesises its own muxed A/V files instead — colour bars plus a 440 Hz sine in FLAC inside Matroska, built with the same gst-launch-shaped pipelines the generator uses, written once into the temp directory and reused. That also gives the mono and 5.1 sources the downmix criterion needs, which no fixture would have provided.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation: crates/sub-media/src/audio.rs holds the audio-side types (AudioFormat, AudioBlock, ChannelLayout, ChannelRole, AudioChannels) and the stereo fold; crates/sub-media/src/decode.rs grew an audio branch on the existing uridecodebin so a video file is still demuxed once (decision-4). DecoderOptions gained streams: StreamSelection (Video, VideoAndAudio, AudioOnly) and audio_channels: AudioChannels; Decoder::audio_format() and Decoder::next_audio_block() are the new API, and media.no_audio_stream is a new stable code. The audio appsink negotiates audio/x-raw,format=F32LE,layout=interleaved at the source's own rate and channel count; open() waits for that negotiation, so the sample rate and channel layout are known before the first pull and a file opened AudioOnly without audio fails at open. Block positions are RationalTime at the source sample rate (one unit = one audio frame), seeded from the first buffer's PTS rounded to the nearest frame and then advanced by frames delivered, so they are contiguous and never float.

Downmix gains (module docs carry the table): front pair at unity, front centre and each surround at 1/sqrt(2) (-3 dB, ITU-R BS.775), LFE dropped, any other position folded to silence; mono goes to both outputs at unity. The fold is not normalised afterwards, which is documented — a hot 5.1 source can exceed +/-1.0 and the mixer is where a limiter belongs. An unpositioned layout keeps its first two channels. AudioChannels::Source turns the fold off. Resampling is deliberately not done here; that is TASK-47.

Verification (Linux, no GPU, GStreamer 1.24.2 from a local sysroot): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-media all green (29 unit + 9 audio_decode + 9 decode_fixtures + 7 probe_fixtures + 3 doc-tests), including a cold run with the synthesised media deleted first.

AC evidence:
- AC #1: tests/audio_decode.rs::a_video_files_audio_decodes_to_interleaved_stereo_f32 and ::the_decoded_sample_count_matches_the_files_duration decode a real Matroska A/V file through the second appsink and assert interleaved f32 stereo output, plus that every block starts exactly where the previous one ended at 48000 units per second (no gaps, no floats). ::one_demux_feeds_both_the_video_and_the_audio_branch pulls frames and audio blocks in step off a single uridecodebin.
- AC #2: audio_format() reports sample_rate, source_channels, source_layout and delivered channels; ::a_mono_source_folds_to_both_stereo_channels_at_unity and ::a_five_one_source_folds_with_the_documented_gains decode genuine mono and 5.1 (FL FR FC LFE RL RR) files, assert the reported layout, that the fold is symmetric, and that the peak is 0.8 for mono and 0.8*(1+2/sqrt(2)) = 1.93 for 5.1 — the documented gains, with the LFE contributing nothing. Unit tests in src/audio.rs pin the gain matrix row by row, including side channels, unpositioned layouts and the LFE.
- AC #3: ::the_decoded_sample_count_matches_the_files_duration decodes all audio of a five-second file and asserts exactly 240000 frames, cross-checked against the container duration read back through sub_media::probe and rescaled to 48 kHz. Deviation to note: the file is synthesised by the test rather than taken from fixtures/manifest.json, because no generated fixture carries audio; the media is built with the same gst-launch pipelines scripts/gen-fixtures.sh uses and is lossless FLAC so the count is exact. If a video+audio fixture is later added to the generator, this test can point at it unchanged.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added an audio branch to sub-media's GStreamer decoder so a video file's audio comes off the same demux as its pictures (decision-4): DecoderOptions::streams selects video, audio or both, the new audio appsink negotiates interleaved F32LE at the source's own rate and channels, and Decoder::next_audio_block() hands out blocks whose start is an exact RationalTime in audio frames. New crates/sub-media/src/audio.rs reports the sample rate and channel layout and folds mono and 5.1 to stereo with documented ITU-R BS.775 gains (front at unity, centre and surrounds at -3 dB, LFE dropped), with media.no_audio_stream as a new stable error code. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test -p sub-media: 9 new integration tests decode real Matroska A/V files (stereo, mono, 5.1) and prove the fold, the reported layout and an exact 240000-frame count against the container duration, plus unit tests pinning every gain matrix row.
<!-- SECTION:FINAL_SUMMARY:END -->
