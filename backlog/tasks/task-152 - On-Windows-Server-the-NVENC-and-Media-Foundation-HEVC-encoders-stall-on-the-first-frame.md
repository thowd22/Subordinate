---
id: TASK-152
title: >-
  On Windows Server the NVENC and Media Foundation HEVC encoders stall on the
  first frame
status: To Do
assignee: []
created_date: '2026-09-12 13:53'
labels:
  - export
  - gpu
  - bug
dependencies: []
ordinal: 172000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The export matrix (TASK-143) on the NVIDIA T4 Windows runner, runs 34692539538 and 34695092575: nvh264enc, nvh265enc and mfh265enc each stop at frame 1 of 50 of the 1280x720 sample project and never make another, for every preset, while mfh264enc, x264enc, x265enc, svtav1enc and rav1enc write all fifty in four to nine seconds on the same machine in the same job. It is not the 4K stall of TASK-148 - this is the small sample project, and it stops at the first frame rather than part way.

Two things separate this machine from the ones where the same elements work. It is Windows Server 2022 with the runner in session 0, which has no desktop and no interactive GPU session; and its NVENC elements reach READY and only fail later, which is the shape another investigation has already seen on this runner (NvEncOpenEncodeSessionEx answering NV_ENC_ERR_INVALID_VERSION after the element is started). mfh265enc failing beside them, while mfh264enc does not, suggests the same session or device question rather than a codec one - and mfh265enc writes clean files on yodaddy, a Windows 11 desktop with a real session.

Whatever the cause, the export should not hang: an encoder that cannot open its session should fail the export with a named error before the first frame is composited.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause is identified and recorded: which call fails, in which session, and whether it is the runner or Windows Server itself
- [ ] #2 An encoder that cannot open an encode session is reported as unusable by the probe rather than as ready, so the automatic order passes over it
- [ ] #3 The export matrix nvidia-windows job passes its nvh264enc, nvh265enc and mfh265enc cells, or the task records why that machine cannot run them
<!-- AC:END -->
