---
id: TASK-150
title: >-
  A video file whose media item has never been probed has its audio decoded as
  if it were an audio-only file
status: To Do
assignee: []
created_date: '2026-09-12 09:07'
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
- [ ] #1 An export of a video file whose media item carries no info decodes its audio through GStreamer, and a project whose media has never been probed exports the same file as one whose media has
- [ ] #2 A regression test covers a video source with a codec symphonia does not carry (Opus in Matroska is the case that found this)
- [ ] #3 Where the routing still needs the model, the item is probed before the export rather than assumed to be audio-only
<!-- AC:END -->
