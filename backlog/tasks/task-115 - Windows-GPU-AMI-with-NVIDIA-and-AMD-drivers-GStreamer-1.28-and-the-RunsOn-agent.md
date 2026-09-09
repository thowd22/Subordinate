---
id: TASK-115
title: >-
  Windows GPU AMI with NVIDIA and AMD drivers, GStreamer 1.28 and the RunsOn
  agent
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
labels:
  - infra
  - gpu
milestone: m-8
dependencies:
  - TASK-114
references:
  - >-
    https://docs.aws.amazon.com/AWSEC2/latest/WindowsGuide/install-nvidia-driver.html
  - >-
    https://docs.aws.amazon.com/AWSEC2/latest/WindowsGuide/install-amd-driver.html
priority: medium
ordinal: 135000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
RunsOn ships Linux GPU images but Windows GPU jobs need an AMI with vendor drivers preinstalled. AWS publishes NVIDIA and AMD driver packages for g4dn and g4ad Windows instances. Build the image with EC2 Image Builder or Packer from the RunsOn Windows base so the agent and drivers are present. Note the known AMF DirectX 12 init crash on Radeon Pro V520; GStreamer's amfh264enc uses DirectX 11 and should be unaffected.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An AMI exists per GPU vendor (or one AMI with both drivers) built by a committed Image Builder or Packer definition under infra/
- [ ] #2 runs-on.yml Windows GPU runners reference the AMI and a smoke job on each shows the GPU in dxdiag or nvidia-smi output
- [ ] #3 gst-inspect-1.0 --exists nvh264enc passes on the NVIDIA runner and amfh264enc on the AMD runner
<!-- AC:END -->
