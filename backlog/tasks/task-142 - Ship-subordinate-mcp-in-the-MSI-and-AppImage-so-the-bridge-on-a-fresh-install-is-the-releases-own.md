---
id: TASK-142
title: >-
  Ship subordinate-mcp in the MSI and AppImage so the bridge on a fresh install
  is the release's own
status: To Do
assignee: []
created_date: '2026-09-12 03:42'
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
