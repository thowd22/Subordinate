---
id: TASK-53
title: Waveform generation job and timeline waveform strips
status: Done
assignee:
  - '@opus-task-53'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 16:57'
labels:
  - audio
  - ui
milestone: m-3
dependencies:
  - TASK-25
  - TASK-45
  - TASK-46
references:
  - docs/PLAN.md
priority: medium
ordinal: 74000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Audio clips are edited visually by their waveform.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Job computes min/max peaks at several zoom levels per media item into the sidecar dir
- [x] #2 Timeline draws waveforms on audio clips using cached textures
- [x] #3 Generation is cancellable and resumable like thumbnails
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-media/src/waveform.rs: WaveformOptions (frames_per_peak, levels, decimation) with validate/fingerprint; Peak {min,max} as i16; WaveformLevel + Waveform (manifest metadata + per-level peak files in the sidecar dir, named from the source ContentHash and the options fingerprint, like ThumbnailStrip).
2. Generation: decode audio once with StreamSelection::AudioOnly, fold the samples into base-level min/max peaks by absolute audio-frame position (PeakBuilder), write the base level atomically, write the manifest, then derive every coarser level by decimating the level below and write each as it is finished. Cancellation is checked between audio blocks and between levels; a resumed run reads back whatever level files are already complete and, when the base level is there, does no decoding at all.
3. New stable code media.waveform_failed; exports from sub-media/src/lib.rs; spawn_waveform_job + WaveformJob on the JobService, mirroring spawn_thumbnail_job.
4. sub-ui/src/waveform.rs: ClipWaveform (one level of peaks, ready to paint), WaveformCache keyed by MediaId holding egui TextureHandles built once from the peaks and reused across frames, a level chooser from the timeline zoom, and the uv mapping from a clip's source range onto its media's texture.
5. TimelinePanel owns the cache, prepares textures for the visible audio clips before painting, and paint_clip draws the cached texture inside the clip rectangle.
6. Tests: unit tests for peak building, decimation, level choice, file round-trip, options validation and the uv/level maths; a sub-media integration test that synthesises a WAV with gst-launch (as tests/audio_decode.rs does) and proves a real strip, a second run that decodes nothing, and a cancelled run that resumes.
7. Verify with cargo fmt, clippy pedantic -D warnings and the touched crates' tests under the local GStreamer environment.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation: sub-media/src/waveform.rs holds the job (WaveformOptions, Peak, WaveformLevel, Waveform, WaveformJob, spawn_waveform_job) and a new stable code media.waveform_failed. The audio is decoded once through StreamSelection::AudioOnly in software and folded into base-level min/max peaks by absolute audio-frame position, so a gap in the stream leaves silence in the right place rather than shifting the strip; every coarser level is a decimation of the level below it, which is what makes several zoom levels cost one decode. Peaks are quantised to i16 and written little-endian into <hash>.<fingerprint>.NN.peaks beside a <hash>.<fingerprint>.wave.json manifest, both written through a temporary file and a rename, so a level's exact byte length is a sound test for 'already generated'.

Resumability: the manifest is rewritten after every level, so an interrupted run leaves a usable plan. A resumed run reads back the base level and derives the rest with no decoding at all; a run that finds every level complete does no work beyond one hash and one manifest read. Cancellation is checked before the decode, between decoded audio blocks and between levels, and reports core.cancelled.

UI: sub-ui/src/waveform.rs turns one level of peaks into one egui texture per media item (a picture of the whole source, white and transparent so it can be tinted), and TimelinePanel owns that cache, uploads textures for the visible audio clips before painting and draws each clip's own uv sub-rectangle of its media's texture. A trimmed, split or scrolled clip therefore costs no upload. level_for_zoom/frames_per_pixel pick the level whose peaks land about one per pixel, in exact rational arithmetic.

Scope notes: the application shell does not host the timeline panel yet (app.rs still says the panels land in later tasks), so nothing wires waveform jobs into TimelinePanel::waveforms_mut() at runtime; that wiring belongs with the task that mounts the panel. Nothing in this change touches the audio callback.

Validation (local WSL, GStreamer 1.24 from the user-prefix build, no GPU): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-media -p sub-ui all green — 91 sub-media unit tests (12 new waveform ones), 5 new integration tests in crates/sub-media/tests/waveform_generation.rs that synthesise a 5 s 48 kHz stereo WAV with gst-launch and assert the real pyramid, the no-decode second run, the cancel-and-resume path and the progress events, plus 108 sub-ui unit tests including four new headless-render tests that paint a frame through egui's Context and inspect the shapes it produced. The repository's generated media fixtures cannot be built on this machine (gst-plugins-bad's timecodestamper is missing), which is why the waveform tests synthesise their own WAV instead of relying on them; every other fixture-backed test in sub-media still skips itself here.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the waveform generation job and the timeline's waveform strips. sub-media gains a waveform module that decodes a source's audio once, folds it into a pyramid of min/max peaks at several zoom levels, and writes each level plus a manifest into the project's sidecar directory under names derived from the source hash and the options; generation reports progress, stops on cancellation between audio blocks and between levels, and resumes from whatever is already on disk — deriving the coarser levels from the base level without decoding again. sub-ui gains a waveform cache that turns one level of peaks into one egui texture per media item, and the timeline panel uploads those textures for the visible audio clips and draws each clip's own part of its media's texture. Verified with cargo fmt --check, clippy pedantic -D warnings across the workspace, and cargo test for sub-media and sub-ui: new integration tests synthesise a real WAV and prove the pyramid on disk, the second run that decodes nothing and the cancel-then-resume path, and new headless render tests prove an audio clip is painted with its cached texture, uploaded once and reused.
<!-- SECTION:FINAL_SUMMARY:END -->
