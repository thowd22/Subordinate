---
id: TASK-116
title: >-
  Hardware verification workflow running encoder and decoder checks on GPU
  runners
status: In Progress
assignee:
  - '@opus-task-116'
created_date: '2026-09-09 17:29'
updated_date: '2026-09-11 13:11'
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
- [ ] #4 Job cost stays under 0.25 USD per run on spot (documented in the workflow header)
- [ ] #5 The NVIDIA Linux job measures compositor readback throughput on the 1080p fixture and reports above 60 fps in the job summary (moved from TASK-58)
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
<!-- SECTION:NOTES:END -->
