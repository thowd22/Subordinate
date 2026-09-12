---
id: TASK-142
title: >-
  Ship subordinate-mcp in the MSI and AppImage so the bridge on a fresh install
  is the release's own
status: Done
assignee:
  - '@opus-task-142'
created_date: '2026-09-12 03:42'
updated_date: '2026-09-12 04:21'
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
- [x] #1 The MSI installs subordinate-mcp.exe beside subordinate.exe and the fresh-install job proves an MCP round-trip using only the installed files
- [x] #2 The AppImage and Flatpak expose subordinate-mcp (via AppRun argument or a bundled binary) and docs/mcp-guide.md shows the .mcp.json entry for each package
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented and verified on hosted runners only.

What changed
- packaging/windows/build-msi.ps1 builds -p subordinate-mcp and stages
  subordinate-mcp.exe into bin\, where the generated FilesBin component group
  installs it; main.wxs needed no change. windows-packaging.yml's post-install
  file list now asserts bin\subordinate-mcp.exe.
- packaging/linux/AppRun gained a --mcp entry point, so an agent's .mcp.json
  names the AppImage itself and the bridge starts with the bundle's GStreamer
  environment (which the subordinate-cli it launches needs).
- The Flatpak manifest builds and installs /app/bin/subordinate-mcp.
- packaging/validate.sh asserts the AppRun branch, the staged AppDir binaries
  and the manifest's build and install commands.
- scripts/fresh-install-check.sh/.ps1 gained an MCP stage; the sh adapter
  contract grew a fourth function, sub_mcp, and every adapter in
  fresh-install.yml and in docs/DEVELOPMENT.md was updated.
- docs/mcp-guide.md has a .mcp.json snippet per package.

A real bug the first verification run caught
Run 34672425215 passed on ubuntu-24.04, the Flatpak and the MSI and failed on
fedora-41: timeline.get_state answered "the project has no sequences", and its
reply is in the log *before* the answer to the sequence.create it depends on.
The server dispatches each request as its own task, so a client that pipes a
whole session in at once races its own calls. Both harnesses now send one
request and wait for its reply before the next, the way scripts/mcp-roundtrip.py
already did; the PowerShell side drives the bridge through redirected stdin and
stdout with a bounded read and leaves the bridge's stderr on the console rather
than in a pipe nobody drains.

Verification
- release.yml dry run 34671467064 on task/task-142 (packaging.yml and
  windows-packaging.yml as reusable workflows): all green. The MSI job shows
  `cargo build ... -p subordinate-mcp` and its installed-file assertion now
  including bin\subordinate-mcp.exe.
- fresh-install.yml run 34672425215 against those packages: caught the race
  above (fedora-41 red, the other three green).
- fresh-install.yml run 34672639050, same packages, harness fixed: AppImage on
  ubuntu-24.04 and fedora-41, Flatpak on ubuntu-26.04 and the MSI on
  windows-latest all pass, each recording
  `mcp=round-trip ok (project.new, sequence.create, timeline.get_state)` with
  the bridge, the engine it launched and the timeline it returned all coming
  from the installed package.
- packaging/validate.sh passes locally.
- hardware=false throughout: no GPU runners, no self-hosted machines.

Criterion 3 is deliberately unchecked: it waits for a release and a Windows
desktop AMI rebuild. gpu-smoke.yml's build-mcp-windows job and
infra/images/windows-desktop/jobscripts/mcp-roundtrip.ps1's fallback are left
in place for exactly that reason -- the *released* MSI (v0.1.1) still has no
bridge, and both already prefer the installed one when it is there.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every package now carries the MCP bridge: the MSI builds and installs subordinate-mcp.exe in bin beside subordinate.exe and the subordinate-cli.exe the bridge falls back to; the AppImage exposes its bundled copy through a new AppRun --mcp entry point; the Flatpak builds and installs /app/bin/subordinate-mcp, reached with flatpak run --command=subordinate-mcp. packaging/validate.sh asserts all three and docs/mcp-guide.md gains a .mcp.json snippet per package. The fresh-install harness proves it rather than asserting it: the adapter contract grew a fourth function, sub_mcp, and both check scripts speak the stdio transport by hand (initialize, project.new, sequence.create, timeline.get_state, one request at a time) with no jq, python or MCP client library, requiring the timeline that comes back to carry the sequence the mutation just made. Verified with real dispatches from the task branch: release.yml dry run 34671467064 built all three packages green; fresh-install.yml run 34672425215 against them caught a genuine bug (the bridge answers requests concurrently, so piping a whole session in raced timeline.get_state ahead of sequence.create; fedora-41 red, the other three green by luck); after the fix, fresh-install.yml run 34672639050 passed on all four hosted legs (AppImage on ubuntu-24.04 and fedora-41, Flatpak on ubuntu-26.04, MSI on windows-latest), each recording mcp=round-trip ok from the installed files alone. Criteria 1 and 2 are checked; 3 waits for a release and the Windows desktop AMI rebuild.
<!-- SECTION:FINAL_SUMMARY:END -->
