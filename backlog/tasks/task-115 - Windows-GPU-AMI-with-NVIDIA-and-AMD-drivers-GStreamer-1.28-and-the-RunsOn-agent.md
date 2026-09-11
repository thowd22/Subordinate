---
id: TASK-115
title: 'Windows GPU AMI with NVIDIA drivers, GStreamer 1.28 and the RunsOn agent'
status: Done
assignee:
  - '@opus-task-115'
created_date: '2026-09-09 17:29'
updated_date: '2026-09-11 17:15'
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
- [x] #1 A usable NVIDIA Windows GPU runner exists in .github/runs-on.yml: a built-in RunsOn Windows image on a g4dn.xlarge family, with no custom AMI required (or, if an in-job driver install proves impossible, a committed Image Builder/Packer definition under infra/)
- [x] #2 A smoke job on that runner shows the GPU in nvidia-smi output and passes gst-inspect-1.0 --exists for nvh264enc (NVENC) and mfh264enc (Media Foundation)
- [x] #3 Cost per run and NVIDIA driver install time are measured from a real run and documented in docs/DEVELOPMENT.md 'GPU CI (RunsOn)'
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Confirm via RunsOn docs that a built-in Windows image (windows22-full-x64) can be paired with a g4dn.xlarge family - no custom AMI needed if the NVIDIA driver installs in-job.
2. Repoint gpu-nvidia-windows in .github/runs-on.yml at windows22-full-x64 + family g4dn.xlarge; delete the two placeholder image entries and the gpu-amd-windows runner (AWS has retired g4ad / has no AMD GPU instance).
3. Add an nvidia-windows job to .github/workflows/gpu-smoke.yml: pull the AWS-published NVIDIA driver from s3://ec2-windows-nvidia-drivers/latest/ (AWS CLI via the instance role, Read-S3Object fallback), install with -s -n (no reboot), run nvidia-smi, install GStreamer 1.28.6 with the same official-installer recipe as ci.yml, then gst-inspect-1.0 --exists nvh264enc and mfh264enc. timeout-minutes: 20.
4. Push the branch with a temporary push trigger, dispatch the job, measure driver-install time and wall clock, remove the temporary trigger.
5. Record cost per run and driver-install time in docs/DEVELOPMENT.md 'GPU CI (RunsOn)' and in the task notes. Budget ~1 USD; if the in-job driver install fails twice, stop and document what an Image Builder pipeline would need.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: AWS no longer offers any AMD GPU instance type (g4ad retired; verified via describe-instance-type-offerings across all regions). AMD parts of this task need Azure NVads V710 v5 or a user-owned AMD box; NVIDIA parts proceed on RunsOn g4dn.

2026-09-09: scope reduced to NVIDIA only. AMD Windows (AMF) has no cloud host: AWS retired g4ad and box runs Linux. AMF verification stays manual until an AMD Windows machine is available.

2026-09-11: re-scoped per supervisor - no custom AMI. RunsOn's stock windows22-full-x64 image runs on a g4dn instance; the missing piece is only the NVIDIA driver, which the job installs from the AWS public bucket s3://ec2-windows-nvidia-drivers with '-s -n' (silent, no reboot). Also removed the gpu-amd-windows runner and both placeholder image entries: AWS offers no AMD GPU instance type at all.

Gotcha found while verifying (run 34617772420, failed in 53s, no instance launched, $0): for PUBLIC repos RunsOn reads .github/runs-on.yml from the DEFAULT branch only, so a runner definition added on a feature branch is invisible - the job still resolved gpu-nvidia-windows to the old placeholder AMI and failed with 'InvalidAMIID.Malformed: ami-00000000000000000'. Branch verification therefore uses the equivalent inline label (image=windows22-full-x64/family=g4dn.xlarge/spot=false) and is switched back to runner=gpu-nvidia-windows before merge.

2026-09-11 verification. Three runs, all on branch task/task-115 (temporary push trigger, removed before the final commit):

- run 34617772420 - failed in 53s, no instance, $0. RunsOn resolved runner=gpu-nvidia-windows against main's config (public-repo rule) and hit the old placeholder AMI. Switched the branch job to the equivalent inline label.
- run 34617932919 - instance launched (i-0874a96587257fa71, g4dn.xlarge on-demand, ami-0142806d2d4aadc50 = windows22-full-x64), driver step failed: 'aws s3 cp ... AccessDenied on ListObjectsV2'. The RunsOn instance role is scoped to the stack's own buckets. ~5.5 min instance, ~$0.07.
- run 34618546433, job 103326431871 - SUCCESS. Fetching the driver over plain anonymous HTTPS works (the bucket is world-listable and world-readable, no credentials at all).

Measured on the successful run:
- Instance launched 15:50:54Z, job started 15:55:00Z -> 4m06s Windows boot + runner registration.
- Driver step 15:55:13 -> 15:56:53 = 100s total: 713 MB downloaded in 14s, 596.86 grid driver installed in ~86s, installer exit code 0, NO REBOOT. nvidia-smi in the very next step reports 'Tesla T4 ... WDDM ... 15360MiB', driver 596.86, CUDA 13.2.
- GStreamer 1.28.6 official MSVC installer: 79s.
- Element check: gst-inspect-1.0 version 1.28.6, pkg-config 1.28.6, 'present nvh264enc' (NVENC H.264 Video Encoder CUDA Mode, gstnvcodec.dll) and 'present mfh264enc'. Media Foundation is available on the stock Windows Server 2022 image - no extra feature install needed.
- Job wall clock 3m22s; billed instance time ~7m30s at the g4dn.xlarge Windows on-demand rate (~$0.752/h) = ~0.10 USD per run. Total spent across all three attempts ~0.17 USD.

Caveat for the merge: because RunsOn reads .github/runs-on.yml from the default branch only, the verification used the inline label image=windows22-full-x64/family=g4dn.xlarge/spot=false, which is exactly what the committed gpu-nvidia-windows definition resolves to. The by-name form goes live the moment this lands on main; re-dispatch GPU smoke once after merge to confirm name resolution.

2026-09-11 supervisor: named runner gpu-nvidia-windows resolved from main on the third dispatch (run 34625543561, all three GPU smoke jobs green; Windows driver ready in 110 s). The first two dispatches after the merge still hit the deleted placeholder AMI, so RunsOn caches .github/runs-on.yml for roughly an hour after a change on the default branch.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Dropped the custom-AMI approach: no Image Builder or Packer pipeline is needed. RunsOn's stock windows22-full-x64 image boots on a g4dn.xlarge and the only missing piece, the NVIDIA driver, installs inside the job in 100 seconds without a reboot.

Changes: .github/runs-on.yml points gpu-nvidia-windows at windows22-full-x64 and drops both placeholder images: entries plus the gpu-amd-windows runner (AWS has no AMD GPU instance type at all); .github/workflows/gpu-smoke.yml gains an nvidia-windows job that pulls the AWS-published driver from the anonymously readable s3://ec2-windows-nvidia-drivers over plain HTTPS, installs it with -s -n, installs GStreamer 1.28.6 with ci.yml's official-installer recipe and asserts nvh264enc and mfh264enc; the hardware.yml 'deliberately absent' comment now says how to add the real Windows job; docs/DEVELOPMENT.md carries the runner table, the recipe, the measured cost and the 'RunsOn reads runs-on.yml from main, not your branch' gotcha.

Verified by run 34618546433 / job 103326431871 on a real g4dn.xlarge Windows Server 2022 instance: nvidia-smi reports Tesla T4 with driver 596.86, gst-inspect-1.0 reports 1.28.6 and both nvh264enc and mfh264enc present, installer exit code 0 with no restart. Driver install 100s (14s download of 713 MB + 86s install); whole job 3m22s; ~7m30s of billed instance time, about 0.10 USD per run; 0.17 USD spent across all three attempts.

Two findings are recorded in the notes and the docs: the AWS-documented 'aws s3 cp' route fails because the RunsOn instance role is scoped to the stack's own buckets, and for public repos RunsOn only reads .github/runs-on.yml from the default branch, so the runner was verified by its inline-label equivalent and the by-name form goes live on merge.
<!-- SECTION:FINAL_SUMMARY:END -->
