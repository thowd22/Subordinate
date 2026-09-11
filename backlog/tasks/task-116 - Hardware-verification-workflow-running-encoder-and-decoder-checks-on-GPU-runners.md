---
id: TASK-116
title: >-
  Hardware verification workflow running encoder and decoder checks on GPU
  runners
status: In Progress
assignee:
  - '@opus-task-116'
created_date: '2026-09-09 17:29'
updated_date: '2026-09-11 14:47'
labels:
  - infra
  - gpu
  - export
  - media
milestone: m-8
dependencies:
  - TASK-114
references:
  - docs/PLAN.md
priority: high
ordinal: 136000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Turns the manual verify tasks into a repeatable, agent-triggerable workflow. Runs on demand and nightly, renders the sample project with each hardware encoder via subordinate-cli, probes hardware decode of the 4K fixture, and uploads outputs and diagnostics as artifacts so criteria can be checked from logs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 hardware.yml runs on workflow_dispatch and a nightly schedule with a job per runner: NVIDIA Linux, AMD Linux, NVIDIA Windows, AMD Windows
- [ ] #2 Each job renders the sample project with the vendor encoder through subordinate-cli render, validates the output with the discoverer, and uploads the file plus gst-inspect diagnostics as artifacts
- [ ] #3 Each Linux job measures 4K H.264 scrub rate with hardware decode using the benchmark harness and prints it in the summary
- [x] #4 Job cost stays under 0.25 USD per run on spot (documented in the workflow header)
- [x] #5 The NVIDIA Linux job measures compositor readback throughput on the 1080p fixture and reports above 60 fps in the job summary (moved from TASK-58)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add .github/workflows/hardware.yml on workflow_dispatch plus one nightly schedule, with a cost header (g4dn.xlarge spot ~0.20 USD/h, 20-minute cap => well under 0.25 USD/run; box is free).
2. Job nvidia-linux on RunsOn gpu-nvidia-linux: install GStreamer 1.24 from apt (24.04 image) plus Vulkan loader and xvfb, pin Rust via rust-toolchain.toml, Swatinem/rust-cache shared-key hardware-nvidia, build only subordinate-bench (release) and subordinate-cli.
3. Job amd-linux on the self-hosted box (self-hosted, linux, box, amd-gpu): validate the preinstalled GStreamer 1.28 and VA-API rather than installing, same build steps, free so no cost cap needed.
4. Each job: gst-inspect diagnostics to a file; scripts/gen-fixtures.sh; scripts/get-sample-media.sh; subordinate-cli render examples/sample-project/demo.sub --preset youtube-1080p --encoder nvh264enc|vah264enc --verify; gst-discoverer-1.0 on the output.
5. Each job: subordinate-bench --out target/bench/perf.json for the 4K H.264 scrub with hardware decode, and cargo test --release -p sub-render --test readback -- --ignored for compositor readback throughput on a 1080p canvas. A python step reads perf.json, prints fixture/decoder/fps into GITHUB_STEP_SUMMARY and fails the job when 4K scrub is below 30 fps or (NVIDIA) readback below 60 fps.
6. Upload the rendered file, perf.json, the discoverer report and gst-inspect diagnostics as artifacts.
7. Leave a commented placeholder for the Windows jobs (no AMD Windows hardware, no NVIDIA Windows AMI until TASK-115).
8. Verify by pushing the branch and dispatching the workflow, at most three attempts and under ~1 USD of GPU time; record run ids and measured numbers in the notes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: AWS no longer offers any AMD GPU instance type (g4ad retired; verified via describe-instance-type-offerings across all regions). AMD parts of this task need Azure NVads V710 v5 or a user-owned AMD box; NVIDIA parts proceed on RunsOn g4dn.

2026-09-09: AMD Linux job runs on the self-hosted box runner (labels self-hosted, linux, box, amd-gpu), not RunsOn; it is free, so it may also run on every push to main if kept under a few minutes. NVIDIA jobs stay on RunsOn gpu-nvidia-linux (workflow_dispatch + nightly). AMD Windows job dropped until hardware exists.

2026-09-11 (opus-task-116): .github/workflows/hardware.yml added with three jobs - build-linux (free hosted ubuntu-24.04, builds subordinate-bench release, subordinate-cli debug and the sub-render readback test binary), nvidia-linux (RunsOn gpu-nvidia-linux) and amd-linux (self-hosted box) - plus scripts/hardware-summary.py, which renders perf.json and the readback log into the job summary and gates on the numbers. Verified by six real runs from branch task/task-116 (a temporary branch push trigger, since a workflow absent from the default branch cannot be dispatched: 'gh workflow run' answers 404; the trigger was removed in the final commit and workflow_dispatch itself is therefore unproven until this lands on main).

Run ids: 34603245414 (GPU job timed out compiling), 34605526227 (cancelled), 34606570134, 34608554627, 34610600133, 34611438521 (NVIDIA job green, 2m17s).

Measured on run 34611438521, Tesla T4 (g4dn.xlarge, driver 580.173.02, Vulkan), Ubuntu 24.04, apt GStreamer 1.24, release bench:
- sample project rendered with nvh264enc: 350 frames, 20.1 MB, gst-discoverer-1.0 reports H.264 + AAC (voaacenc);
- 4K H.264 scrub 8.284 fps through nvh264dec (hardware decoder confirmed by the element name in perf.json), p50 111.970 ms, p95 219.130 ms;
- 4K playback 172.336 fps; 1080p playback 541.235 fps; 1080p scrub 27.168 fps;
- compositor readback on a 1080p canvas 622.0 fps (588.7 and 611.7 on the two runs before it), well past the 60 fps of AC 5.

Cost (AC 4): nothing is compiled on the GPU instance - a cold debug subordinate-cli there cost 10m52s on 4 vCPU and blew the first job's budget - so the binaries come from a free hosted runner of the same distro release. With that plus the in-region EC2 apt mirror and a .deb cache the GPU job is 2m17s: about 0.04 USD on-demand (0.526 USD/h) and under 0.02 USD on spot, against the 0.25 USD the criterion allows. timeout-minutes is 15, a 0.13 USD worst case. The arithmetic is in the workflow header. All instances so far came up on-demand, never spot.

Hardware caveats and findings:
- 4K scrub is far below the plan's 30 fps even with nvdec, and barely above the software baseline (6.9 fps), so the limit is the seek and decode-forward, not the decoder. The workflow prints the number and warns rather than failing; closing the gap belongs to TASK-64/TASK-74. Recorded in docs/PERFORMANCE.md.
- GStreamer registers hardware encoders at rank NONE (autoplug never picks an encoder) and sub-export refuses a deranked element with export.encoder_unavailable, so the render steps set GST_PLUGIN_FEATURE_RANK for the element they pin. Worth deciding whether an explicitly pinned encoder should bypass the derank check in sub-export.
- box (the AMD runner) has the VA-API stack but no Vulkan driver: wgpu reports 'vkCreateInstance: Found no drivers!' and every render fails with render.no_adapter. The job now checks for an ICD up front and fails with that instruction. box also needs rustup's shims on the runner service PATH (added in the job).
- subordinate-bench resolves its default fixtures directory from the path it was compiled in, so a binary built elsewhere must be given --fixtures.
- gst-discoverer-1.0 lives in gstreamer1.0-plugins-base-apps, not gstreamer1.0-tools.
- demo.sub has two sequences, so render needs --sequence 'Main cut'.

Criteria left unchecked, with reasons:
- AC 1: two of the four jobs exist and were proved by real runs (NVIDIA Linux, AMD Linux). There is no NVIDIA Windows AMI (TASK-115) and no AMD Windows hardware anywhere this project can reach, so both Windows jobs are a commented placeholder rather than a job that would quietly pass on software. workflow_dispatch and the nightly schedule are declared but could not be exercised from a branch: GitHub refuses to dispatch a workflow that is not on the default branch, so they will first fire once this merges.
- AC 2: proved on the NVIDIA job only (render, discoverer, artifacts). The AMD job cannot render until box has a Vulkan driver installed (mesa-vulkan-drivers); the job fails fast with that message.
- AC 3: proved on the NVIDIA job (8.284 fps through nvh264dec, printed in the job summary). The AMD job has never reached the benchmark step for the same reason.
<!-- SECTION:NOTES:END -->

## Comments

<!-- COMMENTS:BEGIN -->
author: @opus-task-116
created: 2026-09-11 14:47
---
Left In Progress rather than Done: three of the five criteria cannot be proved from here. AC 2 and AC 3 need mesa-vulkan-drivers installed on box (one apt install by its owner, then re-run the workflow), and AC 1 needs the workflow on the default branch before workflow_dispatch and the schedule can fire at all. Everything on the NVIDIA side is proved and green (run 34611438521).
---
<!-- COMMENTS:END -->
