---
id: TASK-110
title: Fresh-machine install verification on all three OSes
status: In Progress
assignee:
  - '@opus-task-110'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 19:46'
labels:
  - release
  - verify
milestone: m-7
dependencies:
  - TASK-107
  - TASK-109
references:
  - docs/PLAN.md
priority: high
ordinal: 131000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Phase 7 exit criterion.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 On clean Ubuntu, Windows and macOS machines the package installs, opens the sample project, plays with audio and exports with the best available encoder
- [ ] #2 Each run is recorded with OS version, GPU and result in a backlog doc
- [ ] #3 Any failure becomes a blocking task before release
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Scope: Linux (AppImage + Flatpak) and Windows (MSI) now; macOS deferred with TASK-105/117.
2. Add .github/workflows/fresh-install.yml: workflow_dispatch + workflow_call, inputs release_run_id (default: latest successful release.yml run) and hardware ('false' by default).
3. resolve job: pick the release run, assert its subordinate-appimage/-flatpak/-msi artifacts exist, output run id, url, mode (tag or dry run) and head sha. Nothing is ever checked out on a fresh machine: the sample project and the media script are fetched by raw URL at that sha, so the project matches the packages.
4. appimage job (hosted ubuntu-26.04 host, matrix ubuntu:24.04 and fedora:41 containers): the host only downloads the artifact with gh run download and fetches the sample media, then hands a staging directory to a container that has never seen the project. Inside: install only the desktop client libraries a bare container lacks (graphics/audio/X, Mesa software drivers, Xvfb), extract the AppImage once, then open the project through the bundled CLI, probe the media with the bundled gst-discoverer, pick the best H.264 encoder the bundled registry offers (nvh264enc, vah264enc, amfh264enc, mfh264enc, x264enc in sub-export's order, falling back when one refuses to run), render, validate the output with the bundled discoverer, and finally open the project in the real GUI under Xvfb (subordinate --ui-smoke).
5. flatpak job (hosted ubuntu-26.04, no container): install flatpak itself, add flathub, install the bundle from the artifact and run the same open/probe/export/validate sequence through flatpak run --command=.
6. msi job (windows-latest): assert the machine has no GStreamer, download only the MSI, msiexec /i /qn, run the whole sequence from %ProgramFiles%\Subordinate\bin with a PATH of the installed bin plus Windows only, then msiexec /x and assert nothing survives.
7. Every job reports OS version, kernel/glibc or build, GPU (none on hosted runners) and its result into the job summary; a report job aggregates the table.
8. Optional self-hosted jobs behind hardware == 'true': box (AMD, Linux, AppImage) and yodaddy (the user's Windows desktop, MSI) - everything under RUNNER_TEMP, uninstall in an always() step, skipped by default so the standard run stays hosted-only and short.
9. Add a 'Fresh-machine install verification' section to docs/DEVELOPMENT.md with the runbook for a physical fresh machine and for the self-hosted jobs.
10. Record each verification run (OS version, GPU, result, run id) in a backlog doc.
11. Verify with real runs from task/task-110 using a temporary push trigger, remove the trigger before finishing.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor: macOS half of the fresh-install check is deferred with TASK-105/117; Linux and Windows proceed.

2026-09-11 supervisor: dependency on TASK-106 dropped; use the release.yml dry-run artifacts (or the packaging workflows' artifacts) for the Linux and Windows fresh-machine installs. macOS half deferred with TASK-105.
<!-- SECTION:NOTES:END -->
