---
id: TASK-138
title: >-
  Windows NVIDIA desktop AMI: interactive session with the NVIDIA driver,
  Subordinate MSI and test media baked in
status: In Progress
assignee:
  - '@opus-task-138'
created_date: '2026-09-11 22:18'
updated_date: '2026-09-11 22:47'
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
- [ ] #4 subordinate-mcp from the same release is installed on the image and a job proves an MCP call (project.new then timeline.get_state) round-trips against the running app while its window is visible
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Recon job on the stock RunsOn windows22-full-x64 AMI (inline label): dump RUNS_ON env, C:/runs-on and C:/actions-runner contents, services, scheduled tasks, the session id the job runs in, and whether a console session exists; also prototype the AMI steps in-job (local user plus autologon regkeys, MSI silent install, pywinauto, S3 media pull).

2. Decide the interactive-session mechanism from the recon. Preferred: the RunsOn agent started from the auto-logon user logon task. Fallback (documented): agent stays in session 0 and the AMI bakes an interactive helper loop started by a logon scheduled task, driven by a queue directory of PowerShell scripts.

3. Write infra/images/windows-desktop/: Image Builder component (PowerShell steps) for NVIDIA driver, test user with AutoAdminLogon and no lock or screensaver, interactive agent wiring, latest v release MSI silent install, Python plus pywinauto, test MKV from the private S3 bucket, RDP enabled, random password written to Secrets Manager; plus recipe, infrastructure config, distribution config, pipeline JSON and a build script and runbook.

4. Create the AWS resources with the AWS CLI from the committed JSON (instance profile with S3 read, Secrets Manager write, SSM core), run the pipeline once, record the AMI id.

5. Register the AMI in .github/runs-on.yml as gpu-nvidia-desktop-windows and add a job at the end of .github/workflows/gpu-smoke.yml: nvidia-smi, launch Subordinate from the Start Menu shortcut with the sample project, wait for the window, desktop screenshot via CopyFromScreen and upload it, click the media bin Import button through UI Automation, screenshot the file dialog and close it, then an MCP round-trip with subordinate-mcp (project.new then timeline.get_state).

6. Verify by dispatching from task/task-138 with the inline ami label form (named runners only resolve from main), iterate, then remove the temporary push trigger and the recon workflow.

7. Document cost, the 30-day AMI rebuild rule and the rebuild steps in docs/DEVELOPMENT.md; record AMI id, run ids and measured cost in the task notes; check only the criteria a green run proves.
<!-- SECTION:PLAN:END -->
