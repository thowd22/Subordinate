---
id: TASK-104
title: Windows MSI installer with GStreamer runtime
status: In Progress
assignee:
  - '@opus-task-104'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 18:36'
labels:
  - release
milestone: m-7
dependencies:
  - TASK-59
  - TASK-63
references:
  - docs/PLAN.md
priority: high
ordinal: 125000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Windows users expect a single installer.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cargo-wix (or equivalent) builds an MSI bundling the GStreamer MSVC runtime with amf, nvcodec and mf plugins
- [ ] #2 Install, launch, export and uninstall verified on a clean Windows VM
- [ ] #3 Code-signing steps documented even if unsigned for MVP
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. packaging/windows/gst-plugins.txt: Windows plugin allowlist (core, containers, x264/libav, plus nvcodec, amfcodec, mediafoundation, d3d11, wasapi); '!' marks required, same format as the Linux list.
2. packaging/windows/build-msi.ps1: stage ProgramFiles layout (bin/ exes + all GStreamer bin DLLs + MSVC CRT, lib/gstreamer-1.0/ curated plugins, libexec/gstreamer-1.0/gst-plugin-scanner.exe) mirroring the GStreamer prefix so its relocatable prefix detection finds plugins with no env vars; fail when a required plugin is absent; generate a WiX fragment (one component per staged file) and build the MSI.
3. packaging/windows/main.wxs: Package/StandardDirectory ProgramFiles64Folder\Subordinate, Start Menu shortcut, MajorUpgrade, per-machine, WixUI-less silent-capable install.
4. Toolset: WiX v6/v5 via 'dotnet tool install --global wix' rather than cargo-wix (which targets WiX v3, no longer preinstalled on windows-latest images, and cannot harvest a large external runtime tree). Document the choice.
5. .github/workflows/windows-packaging.yml: separate workflow to avoid conflicting with TASK-103's packaging.yml. windows-latest, timeout-minutes 40: install GStreamer 1.28.6 devel silently, cargo build --release of the two binaries, build the MSI, msiexec /i /qn, run installed subordinate.exe --smoke-test, fetch sample media, subordinate-cli.exe render with x264enc from the installed location, validate the output with the bundled gst-discoverer-1.0.exe, msiexec /x /qn and assert the install dir is gone; upload the MSI artifact.
6. docs/DEVELOPMENT.md: Windows MSI section - layout, toolset choice, how to build locally, silent install/uninstall, and the code-signing steps (signtool/Azure Trusted Signing) left undone for MVP.
7. Verify on GitHub windows-latest; record run ids and MSI size.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor: dependencies changed from TASK-64/66 to the export pipeline and CLI render; NVIDIA-on-Windows verification is TASK-115's job once the AMI exists.
<!-- SECTION:NOTES:END -->
