---
id: TASK-148
title: 'vah264enc never finishes a 4K60 export on box, while vah265enc does'
status: To Do
assignee: []
created_date: '2026-09-12 05:32'
labels:
  - export
  - gpu
  - bug
dependencies: []
ordinal: 168000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the export matrix, run 34674014655, on box (AMD Cezanne APU, Ubuntu 26.04, GStreamer 1.28.2, mesa VA-API). Sixty frames of the user 4K60 three-audio-track footage, rendered with subordinate-cli render --encoder vah264enc, produced no file and no error in ten minutes, for all three H.264 presets - youtube-1080p, youtube-4k and mezzanine, which differ only in container and audio codec, so it is the encoder and the canvas that matter and not the preset. The same sixty frames of the same project through vah265enc finished in seconds and validated clean, and vah264enc encodes the 1280x720 sample project fine over all three presets. So it is 4K specifically, on H.264, through VA-API, on this machine.

What makes it worth a task rather than a note: it hangs rather than failing. A user who picks YouTube 4K on an AMD laptop gets a progress bar that stops and never comes back, no error code, no part-written file to inspect and nothing in the log. Whatever the cause - a driver limit the element does not check, a caps negotiation that never completes, a deadlock in the appsrc feed at that frame size - the exporter has to notice and say so.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause is identified from a GST_DEBUG log of the stalled pipeline and recorded in the task: which element stops, at which frame, and why
- [ ] #2 An export that stops making progress fails with a named error naming the encoder and the canvas, rather than hanging, and the part-written file is cleaned up as any other failed export is
- [ ] #3 Where the limit is a real one the encoder cannot exceed, the encoder probe or the export panel says so before the export starts rather than after it stalls
- [ ] #4 The export matrix job on box passes its uhd x vah264enc cells, or the task records why that machine cannot
<!-- AC:END -->
