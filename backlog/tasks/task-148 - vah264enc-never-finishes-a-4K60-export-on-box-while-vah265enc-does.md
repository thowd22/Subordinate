---
id: TASK-148
title: >-
  A 4K60 H.264 export stalls part-way on every hardware encoder tried, on two
  vendors and two operating systems
status: To Do
assignee: []
created_date: '2026-09-12 05:32'
updated_date: '2026-09-12 10:28'
labels:
  - export
  - gpu
  - bug
dependencies: []
ordinal: 168000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the export matrix on two machines that share no code below Subordinate.

box (AMD Cezanne APU, Ubuntu 26.04, GStreamer 1.28.2, mesa VA-API), run 34675906145: sixty frames of the user 4K60 footage with --encoder vah264enc stops at frame 20 of 60 at zero fps for two presets and at frame 1 for the third, and never finishes. The same sixty frames through vah265enc: seconds, clean.

yodaddy (AMD RX 9070 XT, Windows 11, GStreamer 1.28.6, AMF), run 34685591190: the same sixty frames with --encoder amfh264enc stops at frame 22 of 60 for two presets and at frame 1 for the third. The same sixty frames through amfh265enc and amfav1enc: fifteen seconds each, clean, validated.

Different vendor stacks, different operating systems, different drivers, the same shape of failure at the same place - and only for H.264, only at 4K. That points away from the encoders and at the export pipeline: the appsrc feed, the queueing between the compositor readback and the encoder, or h264parse, at a frame size where an H.264 encoder produces enough slices or holds enough reference frames to expose it. Both machines encode the 1280x720 sample project with the same elements over every preset without trouble.

What makes it worth a task rather than a note: it hangs rather than failing. A user who picks YouTube 4K gets a progress bar that stops and never comes back, no error code, no part-written file to inspect and nothing in the log.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause is identified from a GST_DEBUG log of the stalled pipeline and recorded in the task: which element stops, at which frame, and why
- [ ] #2 An export that stops making progress fails with a named error naming the encoder and the canvas, rather than hanging, and the part-written file is cleaned up as any other failed export is
- [ ] #3 Where the limit is a real one the encoder cannot exceed, the encoder probe or the export panel says so before the export starts rather than after it stalls
- [ ] #4 The export matrix job on box passes its uhd x vah264enc cells, or the task records why that machine cannot
- [ ] #5 The cause is identified from a GST_DEBUG log of a stalled pipeline on either machine and recorded in the task: which element stops, at which frame, and why
- [ ] #6 A 4K60 H.264 export finishes on box with vah264enc and on yodaddy with amfh264enc, over every H.264 preset
- [ ] #7 An export that stops making progress fails with a named error naming the encoder and the canvas, rather than hanging, and the part-written file is cleaned up as any other failed export is
- [ ] #8 The export matrix uhd rows for vah264enc and amfh264enc pass
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Run 34675906145 caught where it stops. The CLI prints a progress line per frame, and the last one before the five-minute cap reads "render: 20/60 frames 33% 0 fps eta 44s" for youtube-1080p and youtube-4k, and "render: 1/60 frames 1% 0 fps eta 1212s" for mezzanine. So the pipeline is not slow - it reaches frame 20 (or frame 1) and then makes no progress at all, rate zero, for the remaining minutes. It is a stall, not a throughput problem, and the frame it stalls on is not always the same, which points at the encoder rather than at anything deterministic in the compositor or the decoder. Same sixty frames of the same project through vah265enc: seconds, clean. Reproduce with:

  subordinate-cli render <4k60 project>.sub --sequence UHD --preset youtube-1080p --encoder vah264enc --range 0:60 --out /tmp/x.mp4

with GST_DEBUG=2,va*:6,vah264enc:7 on box.
<!-- SECTION:NOTES:END -->
