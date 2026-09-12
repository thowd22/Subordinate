---
id: TASK-142
title: >-
  Ship subordinate-mcp in the MSI and AppImage so the bridge on a fresh install
  is the release's own
status: In Progress
assignee:
  - '@opus-task-142'
created_date: '2026-09-12 03:42'
updated_date: '2026-09-12 03:46'
labels:
  - release
  - mcp
milestone: m-7
dependencies:
  - TASK-104
  - TASK-103
priority: high
ordinal: 162000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-138 found that packaging/windows/build-msi.ps1 stages only subordinate.exe and subordinate-cli.exe; the Windows desktop image had to build subordinate-mcp on a hosted runner and stage it separately, recorded as mcp_in_msi: false. The MCP bridge is part of the product (docs/PLAN.md §7) and a user installing the MSI should get it, plus a documented .mcp.json snippet pointing at the installed path. Check the AppImage and Flatpak too and add the binary where missing.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The MSI installs subordinate-mcp.exe beside subordinate.exe and the fresh-install job proves an MCP round-trip using only the installed files
- [ ] #2 The AppImage and Flatpak expose subordinate-mcp (via AppRun argument or a bundled binary) and docs/mcp-guide.md shows the .mcp.json entry for each package
- [ ] #3 The Windows desktop image records mcp_in_msi: true after the next release and AMI rebuild
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. MSI: packaging/windows/build-msi.ps1 builds and stages subordinate-mcp.exe alongside subordinate.exe and subordinate-cli.exe (it lands in BINFOLDER through the generated FilesBin component group, so main.wxs needs no change); windows-packaging.yml's post-install file list asserts bin\subordinate-mcp.exe.
2. AppImage: build-appimage.sh already installs usr/bin/subordinate-mcp. AppRun gains an explicit --mcp first argument that execs it (SUB_APPIMAGE_TOOL stays), so "Subordinate.AppImage --mcp" is the documented .mcp.json command. packaging/validate.sh asserts both the AppRun branch and the staged binary.
3. Flatpak: the manifest builds -p subordinate-mcp and installs /app/bin/subordinate-mcp, reached with "flatpak run --command=subordinate-mcp io.github.thowd22.Subordinate".
4. Fresh-install harness: scripts/fresh-install-check.sh and .ps1 gain an MCP round-trip stage run entirely from the installed package -- a stdio JSON-RPC session (initialize, notifications/initialized, project_new, sequence_create, timeline_get_state) against a scratch SUBORDINATE_INSTANCE and SUBORDINATE_ENDPOINT_DIR, with no jq or python dependency, recorded in facts.txt. The sh adapter contract grows a fourth function, sub_mcp; every adapter in .github/workflows/fresh-install.yml (appimage, flatpak, box) and the hand-written one in docs/DEVELOPMENT.md is updated.
5. docs/mcp-guide.md: a .mcp.json snippet per package (MSI path, AppImage --mcp, flatpak run --command), plus the note that the bridge finds subordinate-cli beside itself in each.
6. Verify: push the branch, dispatch release.yml (dry run) from it so packaging.yml and windows-packaging.yml build all three packages with these changes, then dispatch fresh-install.yml with release_run_id set to that run, hosted runners only (hardware false). Record run ids in the notes.
7. Finalize per the guide: check criteria 1 and 2 only; criterion 3 waits for a release and an AMI rebuild.
<!-- SECTION:PLAN:END -->
