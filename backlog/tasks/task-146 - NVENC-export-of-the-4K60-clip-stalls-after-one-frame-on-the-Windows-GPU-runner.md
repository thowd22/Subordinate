---
id: TASK-146
title: NVENC export of the 4K60 clip stalls after one frame on the Windows GPU runner
status: In Progress
assignee:
  - '@opus-task-146'
created_date: '2026-09-12 10:03'
updated_date: '2026-09-12 15:40'
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
- [x] #3 A regression test or hardware-workflow step covers a multi-audio-track 4K source export on Windows
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read TASK-139's notes, scripts/desktop/flow_clicks.py, crates/sub-export (pipeline.rs, sequence.rs, job.rs), crates/sub-media/decode.rs and the Windows GPU workflows; establish what the stalling render actually is (the starter 1080p24 canvas over the 4K60 source, video only, mp4 through nvh264enc, 48 frames).
2. Reproduce on a plain gpu-nvidia-windows runner from a temporary workflow: build subordinate-cli on hosted Windows, install the NVIDIA driver and GStreamer on the GPU runner as gpu-smoke.yml does, fetch the excerpt from S3 with the instance role, build the project with a new scripts/make-test-project.py, and run a matrix of renders under GST_DEBUG with per-experiment timeouts into a log artifact: NVENC alone, NVDEC alone, the failing render with NVENC, the same with x264enc, the same with the Direct3D decoders ranked out, and a 4K60 canvas with three audio tracks.
3. Instrument the export path with a line either side of the composite, the decode and each appsrc push so a log says whether it stalls in the decoder, the readback or the push.
4. Diagnose from the logs, fix in product code, and cover it with a test: the software-encoder render of a multi-audio-track clip runs on hosted Windows, and the hardware step renders the 4K60 three-audio-track excerpt with NVENC.
5. Prove it with a Windows GPU run that writes the full frame count and validates with the discoverer; remove the temporary workflow and record run ids.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Delivered on branch task/task-146 (PR #5). The stall was two faults stacked on one another, and only the first is ours.

THE STALL (ours, fixed). nvh264enc could not open an encode session on the Windows GPU runner: it reached READY, was given caps, answered NV_ENC_ERR_INVALID_VERSION to NvEncOpenEncodeSessionEx, rejected the caps and put not-negotiated on the bus - and nothing in the export path ever read the bus. appsrc's blocking push waits on a condition variable that only a flush wakes, so the render wrote one frame, held the sixth push for the rest of the job, and reported neither an error nor a file. That is exactly the '1/48 frames 2% 1 fps and then nothing' TASK-139 saw. The exporter now applies its own back-pressure: block is off, a push waits for room in 20 ms slices and reads the bus between them. A failed element becomes the error it posted (naming the element), a branch that stops for no stated reason ends the export with export.timeout after a minute, and a branch that is merely slow is still waited for.

THE ENCODER (not ours, worked around). Ruled out on the runner, each with its own experiment: the decoder (NVDEC and NVENC in one gst-launch process encode the clip happily), the wgpu backend (Vulkan and D3D12 both fail), the RGBA readback (gst-launch encodes RGBA with nvh264enc on that machine), the probe's READY cycle, and a second element instance. What is left: subordinate-cli encodes 640x480 with nvh264enc and then cannot open a session for 1920x1080 two seconds later in the same process, while gst-launch encodes 1920x1080 there all day. All three NVENC families behave the same - nvh264enc, nvd3d11h264enc, nvautogpuh264enc - and the Direct3D ones are ranked NONE on that image anyway. mfh264enc, which on an NVIDIA GPU is NVENC behind MediaFoundation, exports the 1080p canvas at 32 fps and is what the automatic order now picks there.

SO THE PROBE WAS ANSWERING THE WRONG QUESTION. It drove each element to READY, which a hardware encoder reaches long before it opens a session. It now encodes a real frame, and the encoder an export is about to plug encodes one frame of the export's own canvas before the pipeline is built - a small frame is not the question either. A pinned encoder that cannot is refused by name and reason; the automatic order walks on to the first that can and says in the log what it left behind.

Also landed: NVENC's Direct3D and auto-GPU elements are catalogued; the video branch's queue holds four frames of its own canvas rather than one (a 4K RGBA frame is 33 177 600 bytes against a 32 MiB cap, so a UHD export used to take turns with its encoder instead of overlapping); scripts/make-test-project.py writes a project over a media file, which is how a GPU job with no window and no bridge gets something to render; and hardware.yml gains the Windows GPU pair its own comment had been asking for since TASK-116, with an `only` input so one job can be dispatched without spending the account's whole quota.

STILL BROKEN, AND NOT BY US: mfh264enc takes about twenty frames of a 4K canvas and then stops taking buffers. It is now an export.timeout with the encoder named rather than a hang, and the hardware job tries it every night in a never-fatal step so the day a driver fixes it the log says so.

Runs. Reproduction and diagnosis on a temporary workflow, all on gpu-nvidia-windows: 34687961764, 34689029628 (the stall caught in the act: 'pushing a video frame frame=6' and no completion, with nvh264enc's not-negotiated on the bus), 34691187378 (the same failures now reported in a second each, job 13m -> 4m31s), 34692603346, 34693482616, 34693730695, 34694572372, 34695454830, 34696234254, 34697380510, 34698326407 (the automatic order falls through to mfh264enc and writes 48 frames), 34699837364 (4K60 with three audio tracks completes on x264enc: 48/48). The temporary workflow is removed.

Proof on the permanent job: hardware.yml run 34700901130, job 103574215608, `only=nvidia-windows`, green in 5m31s. '1080p export: 48 frames on mfh264enc' after 'the chosen encoder cannot encode this canvas; trying the next one element=nvh264enc', and 'UHD export: 48 frames, 38400 audio frames' from the 4K60 canvas with three audio tracks; both read back with the discoverer.

2026-09-12 supervisor: a dedicated agent is working this on branch task/task-146 (it found the mechanism: the export pushes into appsrc with a blocking push and never reads the bus, so a failed element hangs the render). TASK-148 and TASK-152 from the export matrix describe the same stall on other encoders and should be verified against this fix rather than worked separately.

2026-09-12 supervisor handoff: PR #5 (branch task/task-146) holds the fix but was NOT merged: its final CI run 34702935088 failed (see the failing tests recorded here by the next supervisor: run gh run view 34702935088). The agent reported earlier failures were its own two test bugs (fixed on the tip) plus sub-audio's mixer_no_alloc flake on Linux; check whether the remaining failure is that flake, then merge PR #5 manually (git merge --no-ff origin/task/task-146 in the supervisor clone) and reword criterion 1 to 'completes on the encoder the machine can use'. Worktree wf_423097c5-8cf-6 (TASK-151, uncommitted) also edits crates/sub-export/src/pipeline.rs and will conflict.
STATUS AT HANDOFF (stopped at the supervisor's request, branch task/task-146, PR #5, merged up to origin/main at 411633d).

Fixed and proven: the stall. An export whose branch stops draining now reports the element's own error, or export.timeout, instead of hanging - verified on the Windows GPU runner, where the failing job went from 13 minutes of nothing to a named error in one second (run 34691187378), and by crates/sub-export/tests/stalled_branch.rs, which reproduces the stall with software elements and passed on Windows and macOS in CI run 34701467907.

Fixed and proven: the export now completes on that runner. The probe encodes a real frame and the chosen encoder encodes one frame of the export's own canvas first, so selection falls through nvh264enc to mfh264enc. hardware.yml run 34700901130 (job 103574215608, only=nvidia-windows) is green: '1080p export: 48 frames on mfh264enc' and 'UHD export: 48 frames, 38400 audio frames' from a 4K60 canvas with three audio tracks, both read back with the discoverer. AC 3 is checked on that evidence.

Not done: AC 1 as written cannot pass on that runner - nvh264enc cannot open an encode session from this process there, proved five ways (see above), while gst-launch encodes 1920x1080 on the same machine. What passes instead is the same render on the encoder the machine can use. AC 2 is untouched: the desktop flows pin nvh264enc through FLOW_ENCODER, so they will now fail fast with a named error rather than stall; making them green means letting the flow take the automatic order and assert 'a hardware encoder' instead, which is a decision for the supervisor.

Incidental: pinning NV12/I420 between videoconvert and the encoder also fixes the archived TASK-149 (software exports written in High 4:4:4 because nothing pinned a format on the encoder side).

Last CI run on the branch tip is 34702891646, in progress at handoff. The only failure seen on earlier runs that is not fixed on the tip is sub-audio's mixer_no_alloc on Linux, which this branch does not touch.
<!-- SECTION:NOTES:END -->
