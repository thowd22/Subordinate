---
id: TASK-148
title: >-
  A 4K60 H.264 export stalls part-way on every hardware encoder tried, on two
  vendors and two operating systems
status: To Do
assignee: []
created_date: '2026-09-12 05:32'
updated_date: '2026-09-12 19:06'
labels:
  - export
  - gpu
  - bug
dependencies:
  - TASK-146
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

Software H.264 at the same canvas is fine: x264enc writes the same sixty 4K60 frames on box over all three H.264 presets and validates clean (run 34685591190). So it is the hardware H.264 encoders specifically - vah264enc and amfh264enc - and not H.264 at 4K in general, which narrows it to whatever those two have in common that x264enc does not: a DMA or surface-backed buffer pool between the compositor readback and the encoder.

Run 34688535274 adds a third encoder with the identical signature: mfh264enc, Media Foundation on yodaddy, stops at frame 22 of 60 for youtube-1080p and youtube-4k and at frame 1 for mezzanine - the same frames as amfh264enc on the same machine - while mfh265enc writes the same sixty 4K frames and validates clean. Three hardware H.264 encoders now: VA-API on Linux/Mesa, AMF on Windows, Media Foundation on Windows. Three different vendor stacks, two operating systems, one failure, always H.264, always 4K, always around frame 22 or frame 1. x264enc at the same canvas on both machines is fine.

And a fourth, on a fourth vendor: nvh264enc on the Tesla T4 (Ubuntu 24.04, GStreamer 1.24, run 34689001087) stalls on the same sixty 4K60 frames at frame 27, 23 and 9 of 60 for the three H.264 presets, while nvh265enc writes all sixty and validates clean. NVENC, AMF, Media Foundation and VA-API - every hardware H.264 encoder this project can reach, on three operating systems - and x264enc at the same canvas on the same machines is fine. Whatever this is, it is not a driver.

A parallel investigation on another branch has found what turns this into a hang: the export pushes into appsrc with a blocking push and never reads the bus, so an element that stops taking buffers leaves the render waiting on a condition variable that only a flush wakes - no error, no file, no end. Their fix waits for room in slices and reads the bus between them. That changes the symptom this task describes from a hang into an error; whether a 4K H.264 export then *succeeds* on any of these four encoders is the part that remains, and their own note that mfh264enc "takes about twenty frames of a 4K canvas and then stops taking buffers" matches the frame 19 to 27 this matrix measured on all four.

2026-09-12 supervisor: same mechanism as TASK-146 (blocking appsrc push, bus not read). After TASK-146 merges, re-run export-matrix.yml on box and the T4 and close this if vah264enc/nvh264enc finish the 4K60 export.

Integration verification on 2026-09-12, run https://github.com/thowd22/Subordinate/actions/runs/34710769860 at c775441: box passed 17/20 cells, with all three UHD vah264enc presets still timing out after about 147-148 seconds. Yodaddy passed 23/30; six UHD H.264 cells (AMF and Media Foundation, three presets each) still hit the external 120-second cap. A seventh, separate UHD rav1enc preflight regression is being fixed under TASK-151. Hardware HEVC and software x264/x265 UHD cases passed. This run does not establish the remaining H.264 root cause or satisfy the UHD hardware acceptance criteria. Only box/yodaddy and hosted builders ran; no EC2 jobs were requested.
<!-- SECTION:NOTES:END -->
