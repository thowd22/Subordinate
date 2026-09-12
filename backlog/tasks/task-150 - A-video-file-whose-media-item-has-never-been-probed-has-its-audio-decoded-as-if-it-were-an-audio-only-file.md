---
id: TASK-150
title: >-
  A video file whose media item has never been probed has its audio decoded as
  if it were an audio-only file
status: Done
assignee:
  - '@opus-task-150'
created_date: '2026-09-12 09:07'
updated_date: '2026-09-12 15:37'
labels:
  - audio
  - export
  - bug
dependencies: []
ordinal: 170000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
decision-4 splits audio decoding in two: GStreamer decodes the audio inside video files, symphonia decodes audio-only files. sub_export::sequence::decode_audio chooses between them with item.info.as_ref().is_some_and(StreamInfo::has_video) - so a media item whose info is None, which the model documents as the state of an item that has not been probed or was offline when the project was saved, is treated as audio-only and goes to symphonia. The export matrix (TASK-143) hit it head on: a Matroska 4K60 clip with three Opus tracks failed every single cell on yodaddy with "audio.unsupported: building the audio decoder failed: unsupported feature: core (codec): unsupported audio codec" (run 34683736400), because symphonia has no Opus decoder - and the file is a video file that GStreamer would have decoded without complaint.

The symptom is worse than one refused codec. An unprobed video file silently gets a different decoder from a probed one, so whether an export works at all depends on whether the bin happened to have probed the media first. Either the routing should ask the file rather than the model, or an unprobed item should be probed before an export reads it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An export of a video file whose media item carries no info decodes its audio through GStreamer, and a project whose media has never been probed exports the same file as one whose media has
- [x] #2 A regression test covers a video source with a codec symphonia does not carry (Opus in Matroska is the case that found this)
- [x] #3 Where the routing still needs the model, the item is probed before the export rather than assumed to be audio-only
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Replace the model-only routing in sub-export::sequence::clip_pcm: a media item with info decides as before; an item with no info is probed with sub_media::probe_with (frame-timing scan off) before the export reads it, so an unprobed video file goes to GStreamer like a probed one.
2. Memoise the answer per absolute path across the clips of one mix so a project with many clips over one file probes once.
3. Fall back to symphonia (the old behaviour) only when the probe itself fails, with a tracing warning naming the path.
4. Regression test: export a Matroska file with H.264 video and an Opus audio stream, put it in a project as a media item with info: None, and assert SequenceAudio renders non-silent samples (symphonia has no Opus decoder, so the old routing fails this with audio.unsupported). Skip when the machine has no opusenc/x264enc/matroskamux.
5. Verify with cargo fmt, clippy -D warnings and cargo test -p sub-export under the GStreamer env.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Routing now asks the file when the model cannot answer. sub-export::sequence gained source_has_video(item, path, routing): a probed media item still answers from StreamInfo::has_video (no file touched), and an item whose info is None is probed with sub_media::probe_with (scan_frame_timing off, since only the stream lists matter) before the export reads it, so an unprobed video file takes the GStreamer branch exactly like a probed one. The answer is memoised per absolute path for the whole mix, so a sequence that cuts one source many times probes it once, not once per clip.

A probe that itself fails falls back to symphonia — the decoder that file would have got before this change, so a machine whose GStreamer cannot see a format symphonia reads keeps working — and logs a tracing warning naming the path. The decode that follows reports a better error than the probe failure would.

Verification: crates/sub-export/tests/unprobed_media_routing.rs writes the file from the bug report — H.264 pictures and an Opus track in Matroska — and renders the same one-clip sequence twice, once from a project whose media item has never been probed and once from one carrying the probe's answer, asserting a non-silent peak and sample-for-sample equality. Proven to be a real regression test: with source_has_video temporarily reverted to the old model-only routing the test fails with exactly the reported error, SubError { code: audio.unsupported, message: "building the audio decoder failed", cause: "unsupported feature: core (codec): unsupported audio codec" }; with the fix it passes. Two hermetic unit tests in sequence.rs cover the model branch (a probed item answers without touching a nonexistent path) and the per-file cache.

Checks: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-export green (69 lib, 6 job, 4 roundtrip, 1 pinned, 1 new routing test, 4 doctests) under the machine's GStreamer env.

Follow-up worth filing (not created here, parallel branches collide on task ids): import and relink set item.info, but nothing else guarantees a project loaded with unprobed media gets probed, so the UI/CLI could probe media items lazily on load rather than leaving the export to do it; and the same info.is_some_and(has_video) shape exists in sub-ui::media_import.rs:430, which is worth a look for the same blind spot.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
sub-export no longer guesses an unprobed media item is audio-only. The audio-decoder split of decision-4 now reads the model when the item has been probed and probes the file itself when it has not, memoising the answer per file for the mix, so an unprobed video source decodes through GStreamer exactly like a probed one; a failing probe falls back to symphonia with a logged warning. Verified by a new regression test that exports Opus-in-Matroska and renders it from both a probed and an unprobed project — it fails with the reported audio.unsupported error against the old routing and passes with the new one — plus two unit tests, with fmt, workspace clippy -D warnings and cargo test -p sub-export all clean.
<!-- SECTION:FINAL_SUMMARY:END -->
