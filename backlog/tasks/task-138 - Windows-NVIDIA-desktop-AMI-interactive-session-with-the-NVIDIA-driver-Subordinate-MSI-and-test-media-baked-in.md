---
id: TASK-138
title: >-
  Windows NVIDIA desktop AMI: interactive session with the NVIDIA driver,
  Subordinate MSI and test media baked in
status: To Do
assignee: []
created_date: '2026-09-11 22:18'
updated_date: '2026-09-11 22:18'
labels:
  - infra
  - gpu
  - ui
  - test
milestone: m-8
dependencies:
  - TASK-115
  - TASK-140
priority: high
ordinal: 158000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Windows human-style testing needs the runner in an interactive desktop session (session 0 cannot show windows or receive input). Build an AMI with EC2 Image Builder from RunsOn's windows22-full-x64 image: NVIDIA driver preinstalled (no per-job 100 s install), auto-logon of a test user with the RunsOn agent started by a logon task instead of a service, GStreamer 1.28 per the MSI's bundled runtime, the latest Subordinate MSI installed, pywinauto or Windows UI Automation tooling, and the user's test MP4 from the private S3 bucket. Verify that the runner job can move the mouse, type, and screenshot the real desktop.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 infra/images/windows-desktop/ holds a committed Image Builder recipe and a workflow that builds the AMI and records its id in .github/runs-on.yml as runner gpu-nvidia-desktop-windows
- [ ] #2 A smoke job on that runner launches the installed Subordinate from the Start Menu entry, captures a desktop screenshot showing the window, and nvidia-smi reports the T4
- [ ] #3 A scripted click on the media bin's Import button opens the native file dialog and the job proves it by screenshot
<!-- AC:END -->
