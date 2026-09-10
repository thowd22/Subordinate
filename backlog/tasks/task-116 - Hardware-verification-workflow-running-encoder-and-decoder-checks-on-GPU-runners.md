---
id: TASK-116
title: >-
  Hardware verification workflow running encoder and decoder checks on GPU
  runners
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
updated_date: '2026-09-10 08:02'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: AWS no longer offers any AMD GPU instance type (g4ad retired; verified via describe-instance-type-offerings across all regions). AMD parts of this task need Azure NVads V710 v5 or a user-owned AMD box; NVIDIA parts proceed on RunsOn g4dn.

2026-09-09: AMD Linux job runs on the self-hosted box runner (labels self-hosted, linux, box, amd-gpu), not RunsOn; it is free, so it may also run on every push to main if kept under a few minutes. NVIDIA jobs stay on RunsOn gpu-nvidia-linux (workflow_dispatch + nightly). AMD Windows job dropped until hardware exists.
<!-- SECTION:NOTES:END -->
