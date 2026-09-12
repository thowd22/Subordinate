---
id: TASK-138
title: >-
  Windows NVIDIA desktop AMI: interactive session with the NVIDIA driver,
  Subordinate MSI and test media baked in
status: In Progress
assignee:
  - '@opus-task-138'
created_date: '2026-09-11 22:18'
updated_date: '2026-09-12 15:32'
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
- [x] #1 infra/images/windows-desktop/ holds a committed Image Builder recipe and a workflow that builds the AMI and records its id in .github/runs-on.yml as runner gpu-nvidia-desktop-windows
- [x] #2 A smoke job on that runner launches the installed Subordinate from the Start Menu entry, captures a desktop screenshot showing the window, and nvidia-smi reports the T4
- [x] #3 A scripted click on the media bin's Import button opens the native file dialog and the job proves it by screenshot
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Recon (2026-09-12). Two cheap jobs on the stock RunsOn windows22-full-x64 image settled the design. Run 34655612366: the RunsOn Windows agent is not a service - user-data starts bootstrap-agent-windows-AMD64.exe, then cmd.exe, Runner.Listener.exe and Runner.Worker.exe, all in session 0, and a job step runs as NT AUTHORITY SYSTEM in session 0. Run 34656029109: the MSI installs silently (exit 0) and pywinauto installs, but the editor cannot run there at all - SwapChain creation error 0x887A0022, In Surface configure, Invalid surface. Combined cost about 0.06 USD.

Why the agent stays in session 0: RunsOn's user data belongs to the control plane, carries the job's JIT runner registration, and the only lever a custom AMI has is overwriting the vendor bootstrap binary at a version-pinned path - which RunsOn's own custom-image docs say to leave alone. The image adds a bridge instead: a local administrator auto-logged onto the console session, an interactive helper started by a logon scheduled task, and a queue directory the job writes scripts into. InteractiveSession.psm1 refuses to run when the helper heartbeat is missing, stale or in session 0.

Sysprep is what broke the image, twice. Builds 1.0.7 and 1.0.9 ran every component to 'validate ok' and then went to EC2 instance status impaired inside RunSysPrepScript and never shut down (1.0.7 sat there 55 minutes before it was killed). The pipeline now runs a custom BUILD workflow, infra/images/windows-desktop/workflow-build-no-sysprep.yml, which is the AWS build-image workflow minus the Sysprep step - and minus InventoryCollection, whose ssm:CreateAssociation failed build 1.1.1 and whose output nothing reads. The last build step runs EC2Launch.exe reset, AWS's documented way to prepare an instance for imaging without generalize. Consequence found the hard way in run 34667886109: with no Sysprep there is no Setup phase, so SetupComplete.cmd never runs and the auto-logon registry values have to be written during the build.

AMI: ami-0b3c82cb19d31ac1d (recipe 1.1.3, built 2026-09-12 03:12 UTC, name subordinate-windows-desktop-2026-09-12T02-43-xx). Parent: RunsOn's ami-0142806d2d4aadc50 (runs-on-v2.2-windows22-full-x64-20260817070000). Contents: NVIDIA GRID driver 596.86 with the Tesla T4 as a display adapter, local administrator subtest auto-logged on at boot, the interactive helper and its logon task, RDP on, the v0.1.1 MSI (Subordinate plus GStreamer 1.28), machine-wide Python 3.12 with pywinauto 0.6.9, and meld-4k60-excerpt-2min.mkv verified by sha256. The subtest password is generated per build into the Secrets Manager secret subordinate/windows-desktop-ami/rdp and is in no file in the repository. The superseded ami-041900754467f620f and its snapshot were deregistered and deleted.

Verification run 34670630488 (branch task/task-138, inline label ami=ami-0b3c82cb19d31ac1d/family=g4dn.xlarge/spot=false, all jobs green, 3m05s in the job). Evidence in the log: nvidia-smi reports 'Tesla T4 WDDM' with driver 596.86 and CUDA 13.2; 'helper: session=1 user=EC2AMAZ-3ULA3DV\subtest desktop=2304x800' next to 'this job: session=0'; the Start Menu shortcut resolves to C:/Program Files/Subordinate/bin/subordinate.exe and the editor reports 'adapter chosen: Tesla T4 (discrete GPU, Vulkan, driver NVIDIA 596.86)' and 'opened C:/SubordinateTest/sample-project/demo.sub'; the desktop screenshot is 2304x800 with 1719 distinct sampled colours; UI Automation found window 'Subordinate' and button 'Import...' and a real mouse click opened the native dialog 'Import media' (class #32770); and project.new, sequence.create and timeline.get_state all round-tripped through subordinate-mcp against that window. The three PNGs are the windows-desktop-screenshots artifact; 01-editor.png shows the loaded timeline and 02-import-dialog.png the open file dialog.

Cost. Seven Image Builder runs on on-demand g4dn.xlarge Windows totalled about 3.4 hours, roughly 2.55 USD - most of it the two runs that hung in Sysprep. The successful build was 29 minutes, about 0.36 USD. Recon and four desktop smoke runs added about 0.48 USD. Total for the task about 3.0 USD against the 4 USD cap. Ongoing: the AMI snapshot is about 1.50 USD a month, so superseded AMIs and their snapshots should be deleted (one already was).

Two things for the supervisor. (1) The released MSI does not carry subordinate-mcp.exe: packaging/windows/build-msi.ps1 stages subordinate.exe and subordinate-cli.exe only, so acceptance criterion 4's 'installed on the image' is not met literally. The image records mcp_in_msi=false in C:/SubordinateTest/image.json, and a free hosted-runner job in gpu-smoke.yml builds the bridge and stages it, which is what proved the round trip. Adding subordinate-mcp to the MSI is a one-line change to build-msi.ps1 plus a release, and belongs to the packaging task. (2) RunsOn reads .github/runs-on.yml from the default branch only, so runner=gpu-nvidia-desktop-windows resolves only once this lands on main; re-dispatch GPU smoke afterwards to confirm (TASK-115 saw RunsOn cache the old config for about an hour).

2026-09-12 supervisor handoff: image 1.1.6 rebuilt from v0.1.3 on 2026-09-12 (see runs-on.yml for the id once updated); criterion 4 (mcp_in_msi true) flips after a release carrying TASK-142 (0.1.4) and another rebuild. Rebuild at least every 30 days; build with SKIP_IAM=1 infra/images/windows-desktop/build.sh <new semver> and poll imagebuilder get-image.
<!-- SECTION:NOTES:END -->

## Comments

<!-- COMMENTS:BEGIN -->
author: @opus-task-138
created: 2026-09-12 03:41
---
Criterion 4 needs a decision: the round trip is proven but subordinate-mcp.exe is not in the release MSI, so it is not installed on the image. Either add it to packaging/windows/build-msi.ps1 and cut a release, then rebuild the AMI, or accept the hosted-runner build the smoke job stages today. Also: runner=gpu-nvidia-desktop-windows only resolves once this is on main - re-dispatch GPU smoke after the merge.
---
<!-- COMMENTS:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Built the Windows NVIDIA desktop runner AMI and proved a job can drive the real application on it. ami-0b3c82cb19d31ac1d, from infra/images/windows-desktop/ - four EC2 Image Builder components, a recipe, infrastructure and distribution configurations, a custom build workflow, the IAM documents and an idempotent build.sh - registered in .github/runs-on.yml as the image behind runner gpu-nvidia-desktop-windows, with the desktop smoke job at the end of .github/workflows/gpu-smoke.yml.

The image carries the NVIDIA driver (Tesla T4 as a display adapter), the released MSI with its GStreamer 1.28 runtime, Python with pywinauto and the 4K test clip, and it logs a local administrator onto the console session at boot. The RunsOn agent stays where RunsOn puts it, as SYSTEM in session 0, which has no desktop and where the editor cannot even create a swapchain; jobs reach the desktop through an interactive helper the image starts from a logon task, driven by a queue directory.

Verified by run 34670630488: nvidia-smi reports the T4 with driver 596.86, the helper answers from session 1 while the job runs in session 0, the Start Menu shortcut opens the editor on the T4 through Vulkan with demo.sub loaded, the desktop screenshot has 1719 distinct colours across 2304x800, UI Automation finds the AccessKit button 'Import...' and a real mouse click opens the native 'Import media' dialog (screenshotted), and project.new, sequence.create and timeline.get_state round-trip through subordinate-mcp against that same window.

Criterion 4 is left unchecked on purpose: the round trip is proven, but the released MSI does not ship subordinate-mcp.exe (build-msi.ps1 stages only subordinate.exe and subordinate-cli.exe), so the bridge is not on the image - a free hosted-runner job builds and stages it instead, and the image records mcp_in_msi=false. Cost about 3.0 USD of the 4 USD budget, most of it two builds that hung in Sysprep before that step was removed from the pipeline.
<!-- SECTION:FINAL_SUMMARY:END -->
