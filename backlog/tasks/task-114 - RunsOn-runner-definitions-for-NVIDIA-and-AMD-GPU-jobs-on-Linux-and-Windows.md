---
id: TASK-114
title: RunsOn runner definitions for NVIDIA and AMD GPU jobs on Linux and Windows
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
labels:
  - infra
  - gpu
milestone: m-8
dependencies:
  - TASK-112
  - TASK-113
references:
  - 'https://runs-on.com/runners/gpu/'
priority: high
ordinal: 134000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Workflows should reference stable runner names rather than raw EC2 parameters. RunsOn reads .github/runs-on.yml for named runners and images. Linux GPU images (ubuntu24-gpu-x64) ship with RunsOn; Windows GPU needs a custom AMI (separate task), so Windows runners point at that AMI once it exists.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 .github/runs-on.yml defines runners gpu-nvidia-linux (g4dn.xlarge, ubuntu24-gpu-x64, spot preferred), gpu-amd-linux (g4ad.xlarge), gpu-nvidia-windows and gpu-amd-windows (custom AMI placeholder), each with a cost cap
- [ ] #2 A workflow_dispatch smoke job on gpu-nvidia-linux prints nvidia-smi and gst-inspect-1.0 --exists nvh264enc succeeds
- [ ] #3 A workflow_dispatch smoke job on gpu-amd-linux prints the DRM device and vainfo, and gst-inspect-1.0 --exists vah264enc succeeds after installing the va plugin
<!-- AC:END -->
