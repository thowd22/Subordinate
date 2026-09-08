---
id: decision-4
title: 'GStreamer decodes audio inside video files, symphonia decodes audio-only files'
date: '2026-09-08 20:52'
status: accepted
---
## Context

Video files should be demuxed once, but audio-only files do not need a GStreamer pipeline, and the playback mixer must be owned by the app for real-time safety.

## Decision

GStreamer decodes audio tracks inside video files. symphonia decodes audio-only files. Both feed the same owned lock-free mixer graph driven by cpal.

## Consequences

Two decode paths to test. The mixer never depends on GStreamer's clock; the audio callback is the playback master.
