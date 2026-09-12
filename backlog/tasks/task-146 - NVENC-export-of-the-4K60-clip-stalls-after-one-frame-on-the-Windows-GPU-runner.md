---
id: TASK-146
title: NVENC export of the 4K60 clip stalls after one frame on the Windows GPU runner
status: In Progress
assignee:
  - '@opus-task-146'
created_date: '2026-09-12 10:03'
updated_date: '2026-09-12 10:15'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read TASK-139's notes, scripts/desktop/flow_clicks.py, crates/sub-export (pipeline.rs, sequence.rs, job.rs), crates/sub-media/decode.rs and the Windows GPU workflows; establish what the stalling render actually is (the starter 1080p24 canvas over the 4K60 source, video only, mp4 through nvh264enc, 48 frames).
2. Reproduce on a plain gpu-nvidia-windows runner from a temporary workflow: build subordinate-cli on hosted Windows, install the NVIDIA driver and GStreamer on the GPU runner as gpu-smoke.yml does, fetch the excerpt from S3 with the instance role, build the project with a new scripts/make-test-project.py, and run a matrix of renders under GST_DEBUG with per-experiment timeouts into a log artifact: NVENC alone, NVDEC alone, the failing render with NVENC, the same with x264enc, the same with the Direct3D decoders ranked out, and a 4K60 canvas with three audio tracks.
3. Instrument the export path with a line either side of the composite, the decode and each appsrc push so a log says whether it stalls in the decoder, the readback or the push.
4. Diagnose from the logs, fix in product code, and cover it with a test: the software-encoder render of a multi-audio-track clip runs on hosted Windows, and the hardware step renders the 4K60 three-audio-track excerpt with NVENC.
5. Prove it with a Windows GPU run that writes the full frame count and validates with the discoverer; remove the temporary workflow and record run ids.
<!-- SECTION:PLAN:END -->
