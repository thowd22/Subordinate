---
id: TASK-116
title: >-
  Hardware verification workflow running encoder and decoder checks on GPU
  runners
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
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
<!-- AC:END -->
