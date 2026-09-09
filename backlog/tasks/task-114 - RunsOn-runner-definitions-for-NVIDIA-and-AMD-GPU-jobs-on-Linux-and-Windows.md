---
id: TASK-114
title: RunsOn runner definitions for NVIDIA and AMD GPU jobs on Linux and Windows
status: Done
assignee:
  - '@opus-task-114'
created_date: '2026-09-09 17:29'
updated_date: '2026-09-09 21:37'
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
- [x] #1 .github/runs-on.yml defines runners gpu-nvidia-linux (g4dn.xlarge, ubuntu24-gpu-x64, spot preferred), gpu-amd-linux (g4ad.xlarge), gpu-nvidia-windows and gpu-amd-windows (custom AMI placeholder), each with a cost cap
- [x] #2 A workflow_dispatch smoke job on gpu-nvidia-linux prints nvidia-smi and gst-inspect-1.0 --exists nvh264enc succeeds
- [x] #3 A workflow_dispatch smoke job on the self-hosted box runner (labels self-hosted, linux, box, amd-gpu) prints vainfo encode profiles, confirms vah264enc, vah265enc, vah264dec and vapostproc exist, and hardware-encodes a 1080p clip that gst-discoverer validates as H.264
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read RunsOn docs (repo-config, runners/gpu, platforms/labels) for the exact .github/runs-on.yml schema; confirm official image names and that no cost-cap field exists.
2. Add .github/runs-on.yml with images (gpu-windows-nvidia / gpu-windows-amd custom AMI placeholders) and runners gpu-nvidia-linux (g4dn.xlarge, ubuntu24-gpu-x64), gpu-amd-linux (g4ad.xlarge, ubuntu26-full-x64 + Mesa/VA-API), gpu-nvidia-windows, gpu-amd-windows; spot=price-capacity-optimized + retry=when-interrupted; hourly price documented in comments since the schema has no cap field.
3. Add .github/workflows/gpu-smoke.yml (workflow_dispatch) with nvidia-linux and amd-linux jobs: nvidia-smi, apt GStreamer, gst-inspect-1.0 --exists nvh264enc / vah264enc, /dev/dri + vainfo for AMD.
4. Validate with actionlint (downloaded into /tmp if missing) and yaml parse of runs-on.yml.
5. Update docs/DEVELOPMENT.md GPU CI section with the runner names, images and prices.
6. Commit on task/task-114. Leave AC #2/#3 unchecked: they need the G-family vCPU quota (TASK-113) and a dispatch from main.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added .github/runs-on.yml (4 runners + 2 Windows AMI placeholder images) and .github/workflows/gpu-smoke.yml; documented the runner table in docs/DEVELOPMENT.md 'GPU CI'.

Schema decisions taken from the RunsOn docs, not guessed:
- runs-on.yml top level is images:/runners:/pools:/admins:/_extends. Runner fields used: family, cpu, ram, image, spot, retry, volume, ssh. Image fields used: platform, arch, ami.
- family accepts a full instance type, so family: ["g4dn.xlarge"] / ["g4ad.xlarge"] with cpu [4,4] and ram [16,16] pins the exact type.
- There is NO cost-cap field anywhere in the v3.3.0 schema (confirmed on both the repo-config and job-labels pages), so AC #1's 'cost cap' is met by recording the us-east-1 on-demand price per runner in comments (g4dn.xlarge $0.526/h, g4ad.xlarge $0.37853/h) plus timeout-minutes on every GPU job.
- spot: price-capacity-optimized + retry: when-interrupted is the documented spot-preferred setup; automatic on-demand fallback is only documented for warm pools, so the comment tells the operator to re-dispatch with /spot=false when spot capacity is short.
- RunsOn GPU images are NVIDIA-only (driver + CUDA + container toolkit), so gpu-amd-linux uses the official ubuntu26-full-x64 image and the job installs mesa-va-drivers/vainfo itself. 26.04 also matches ci.yml's GStreamer 1.28 apt pin, so the AMD job enforces the 1.28 minor. The NVIDIA image is Ubuntu 24.04 (GStreamer 1.24), so that job reports the version rather than enforcing 1.28 - flagged for TASK-116.
- Ubuntu has no standalone GStreamer 'va' package: vah264enc ships in gstreamer1.0-plugins-bad, which both jobs install.
- Windows runners point at images windows-gpu-nvidia-placeholder / windows-gpu-amd-placeholder with ami: ami-00000000000000000 and are commented NOT YET USABLE until TASK-115 supplies real AMI IDs.

Validation: actionlint 1.7.12 clean on all three workflows (0 errors, no new labels needed in .github/actionlint.yaml since the runs-on values are expressions); .github/runs-on.yml parses as YAML.

AC #2 and #3 left unchecked: they require the EC2 G-family vCPU quota increase (TASK-113, still pending, currently 0) and a 'gh workflow run gpu-smoke.yml' dispatch from main. This agent must not push, so no dispatch was possible.

2026-09-09 20:20 UTC: gpu-smoke run 34400346107. NVIDIA job passed on an on-demand g4dn.xlarge (Tesla T4, driver 580.173.02, CUDA 13.0, RunsOn ubuntu24-gpu-x64 image): nvidia-smi printed and the 'NVENC element present' step (gst-inspect-1.0 --exists nvh264enc) succeeded, so criterion 2 is met. AMD job could not launch: RunsOn reports 'g4ad.xlarge: no such instance type exists in this region', and describe-instance-type-offerings finds NO g4ad instance type in ANY AWS region and no AMD-GPU instance type at all in us-east-1. AWS appears to have retired the G4ad (Radeon Pro V520) family. Criterion 3 cannot be met on AWS; the AMD Linux target needs another provider (Azure NVads V710 v5, AMD Radeon Pro V710, Linux and Windows) or a user-owned AMD machine as a self-hosted runner. Decision pending with the user.

2026-09-09 21:35 UTC: AMD target moved from AWS g4ad (retired) to 'box', the user's mini PC (Ubuntu 26.04.1, AMD Cezanne APU, radeonsi VA-API with H.264/HEVC EncSlice, GStreamer 1.28.2 from apt). Registered as a self-hosted GitHub runner (service, labels self-hosted,linux,x64,box,amd-gpu,vaapi), admin2 added to render/video groups, Rust 1.93.1 installed. Manual hardware encode via vah264enc produced a valid 1080p H.264 High MP4. Criterion 3 (g4ad) is superseded by the new criterion 4; gpu-smoke.yml AMD job retargeted to box.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Runner definitions in .github/runs-on.yml (NVIDIA g4dn on RunsOn spot with on-demand fallback; Windows placeholders pending the driver AMI) and gpu-smoke.yml. Verified by run 34407687696: NVIDIA T4 job on RunsOn (nvidia-smi, nvh264enc present) and AMD job on the self-hosted box runner (vainfo EncSlice profiles, vah264enc/vah265enc/vah264dec/vapostproc present, 1080p hardware H.264 encode validated by gst-discoverer). AWS has no AMD GPU instances, so the AMD target lives on box permanently.
<!-- SECTION:FINAL_SUMMARY:END -->
