---
id: TASK-144
title: >-
  Wire the viewer and playback to the decoder so the app shows decoded video
  while scrubbing and playing
status: In Progress
assignee:
  - '@opus-task-144'
created_date: '2026-09-12 05:04'
updated_date: '2026-09-12 05:08'
labels:
  - ui
  - media
  - render
  - bug
milestone: m-1
dependencies:
  - TASK-133
  - TASK-56
  - TASK-22
  - TASK-70
priority: high
ordinal: 164000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-133's agent found that nothing in sub-ui or sub-edit calls sub_media::Decoder: the viewer panel draws the compositor texture but no decoded frames feed it, so in the shipped app scrubbing and playback show no video from real media (the only production caller of the seek path is sub-export). The decode pipeline exists and is fast (decoder with hardware preference, decode-ahead ring, frame cache, PTS index, and since TASK-133 a GOP cache giving 45 to 101 fps 4K scrub on real GPUs) but the UI never constructs it. The compositor (TASK-21/56) samples clip frames through an interface that must be backed by per-clip IndexedDecoders driven by the playhead and the playback scheduler (TASK-23/56), off the UI thread, with proxies (TASK-70) when enabled.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Opening the sample project and scrubbing the timeline shows the correct decoded frame in the viewer and the pop-out; the Xvfb window smoke and the Linux desktop smoke screenshots show real picture content (not black) and the desktop smoke asserts it by comparing against a frame rendered by subordinate-cli
- [ ] #2 Play/JKL playback decodes ahead on worker threads with the audio clock as master; dropped frames are counted; the UI thread never blocks on decode
- [ ] #3 Scrubbing uses Decoder::set_index / IndexedDecoder so the GOP cache applies; the hardware workflow's scrub_drag number is measured through the same code path the viewer uses
- [ ] #4 kittest interaction test: scrub to frame N shows the frame whose burned-in timecode is N on a generated fixture
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New crates/sub-ui/src/preview.rs: a PreviewService owning one decode worker per active clip, off the UI thread. Reuses the shape sub-export's SequenceFrames uses for export (resolve_layers_at -> per-clip decoder -> seek to ResolvedClip::source_time -> Nv12Converter -> SourceFrame -> Compositor::render), but made non-blocking and stateful for live preview.
2. Per clip: open on a JobService worker (PtsIndex::load_or_build then Decoder::open_with(NV12) then Decoder::set_index then DecodeAhead::with_decoder). set_index is what makes the TASK-133 GOP cache and the accurate-aim planner apply, which is criterion 3. The opened handle is moved to the UI thread when the job lands; until then the clip has no picture and the viewer shows black for it.
3. Scrub: request(source_time) maps the time to a picture through the index (frame_at / pts) and asks DecodeAhead::seek_to only when the wanted picture is not already the current one and is not within the ring's reach; a forward step the ring can serve is taken from the ring instead, so playback keeps its decode-ahead.
4. Play: the same worker fills its bounded ring ahead of the playhead; the UI pops only while occupancy() > 0, so the UI thread never blocks on decode. Frames the playhead has already run past are popped and counted as dropped, alongside DecodeAheadStats::frames_dropped and PlaybackScheduler::dropped_frames.
5. Proxies: the media path comes from MediaItem::absolute_source(project_dir, viewer.media_use()), which already resolves a Ready proxy only when the viewer's Proxy toggle is on and never for export. Flipping the toggle drops the open decoders so they reopen on the other file.
6. app.rs: composite() stops using the empty frame source. It asks the preview service for the pictures of the layers under the playhead, uploads each through a per-clip Nv12Converter kept between frames (as the bench's Uploader does), and hands them to Compositor::render. A composite is redone when the playhead moved OR a clip delivered a new picture; a repaint is requested while any clip is still opening or decoding.
7. Bound the pool: decoders for clips not under the playhead recently are dropped, with a hard cap on open pipelines.
8. Tests: crates/sub-ui/tests/viewer_decode.rs - a kittest interaction test that builds a one-clip project over the generated bars fixture, scrubs the viewer to frame N, drives the preview to completion and asserts the composited canvas reads back the burned-in timecode of frame N and matches a reference frame decoded straight from the fixture. Plus unit tests in preview.rs for the request/seek decision and the drop counting.
9. scripts/ui-smoke.sh: open a project whose clip has real picture, and assert the editor screenshot is not a flat black viewer (a picture-content check over the viewer rectangle).
10. cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, and the sub-ui, sub-media, sub-render suites. Criterion 1's desktop-smoke half and criterion 3's hardware measurement need GPU runners and are left for the supervisor.
<!-- SECTION:PLAN:END -->
