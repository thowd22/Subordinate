---
id: TASK-146
title: NVENC export of the 4K60 clip stalls after one frame on the Windows GPU runner
status: In Progress
assignee:
  - '@opus-task-146'
created_date: '2026-09-12 10:03'
updated_date: '2026-09-12 15:32'
labels:
  - export
  - bug
  - gpu
milestone: m-4
dependencies:
  - TASK-139
priority: high
ordinal: 166000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-139's Windows desktop clicks flow starts a real nvh264enc export of the baked 4K60 three-audio-track MKV excerpt and it writes one frame then stops (1/48 frames, 2 percent, 1 fps). The agent reproduced it with subordinate-cli render in session 0 with no window or bridge, so it is in sub-export or the decode path on Windows with NVDEC/NVENC, not the harness. The same export completes on the Linux T4 image. Reproduce on runs-on runner gpu-nvidia-windows (or the Windows desktop image via inline ami=ami-0b3c82cb19d31ac1d) with GST_DEBUG on the pipeline; suspects: the appsrc feeding pace versus the encoder's D3D11 memory, the three audio tracks, or the decoder pipeline stalling on the second GOP under Windows.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 subordinate-cli render of the 4K60 excerpt with nvh264enc on the Windows GPU runner completes with the expected frame count and the discoverer validates the file
- [ ] #2 The clicks-only desktop flow on the Windows image reaches its export verdict green (TASK-139 criteria 3 and 4 on Windows)
- [ ] #3 A regression test or hardware-workflow step covers a multi-audio-track 4K source export on Windows
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-12 supervisor: a dedicated agent is working this on branch task/task-146 (it found the mechanism: the export pushes into appsrc with a blocking push and never reads the bus, so a failed element hangs the render). TASK-148 and TASK-152 from the export matrix describe the same stall on other encoders and should be verified against this fix rather than worked separately.
<!-- SECTION:NOTES:END -->
