---
id: TASK-110
title: Fresh-machine install verification on all three OSes
status: Done
assignee:
  - '@opus-task-110'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 20:31'
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
- [x] #1 Each run is recorded with OS version, GPU and result in a backlog doc
- [x] #2 Any failure becomes a blocking task before release
- [x] #3 On clean Ubuntu and Windows machines the package installs, opens the sample project, plays with audio and exports with the best available encoder (macOS moved to TASK-117 until the user's Mac arrives)
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

2026-09-11 implementation (branch task/task-110):

Added .github/workflows/fresh-install.yml plus scripts/fresh-install-check.sh
and scripts/fresh-install-check.ps1, and a "Fresh-machine install
verification" section in docs/DEVELOPMENT.md with the by-hand runbook. Runs
are logged in doc-4.

Design decisions worth keeping:

* The packages come from a release.yml run (gh run download), never rebuilt
  here: a package rebuilt for the test is not the package the user gets.
  `release_run_id` defaults to the latest successful release.yml run. The
  workflow_call trigger is there so release.yml can gate a tag on this later
  by passing `release_run_id: ${{ github.run_id }}`.
* Nothing is ever checked out. A fresh machine has no source tree, so the
  sample project and its media come by raw URL at the commit the packages
  were built from, and the check harness comes at the commit running the
  workflow (the first run, 34641476353, 404ed because the harness does not
  exist at the packages' commit).
* The AppImage legs run in ubuntu:24.04 and fedora:41 containers; the hosted
  runner only carries the file in, like a browser download. The Flatpak leg
  uses the hosted runner itself because flatpak needs bubblewrap and user
  namespaces, which an unprivileged container does not give it.
* The check sequence lives in one script per platform rather than in the
  workflow, reached through a three-function adapter (sub_cli, sub_tool,
  sub_gui), so the by-hand runbook runs exactly what CI runs and a future dmg
  needs only an adapter.
* "Best available encoder" is sub_export's own order, each candidate pinned
  with --encoder and tried in turn: registered is not usable (nvcodec and va
  register elements that need a driver), so a candidate that cannot render
  falls through. Hosted Linux lands on x264enc; Windows picked mfh264enc,
  which without a GPU is a software Media Foundation transform.
* The optional box and yodaddy jobs are behind `hardware=true` and were not
  run, so nothing here claims them. yodaddy's uninstall is if: always() and
  all its files live under RUNNER_TEMP.

Finding, fixed in this branch: the AppImage shipped the GStreamer discoverer
library but not the gst-discoverer-1.0 command, because Ubuntu keeps it in
gstreamer1.0-plugins-base-apps which the build container never installed, and
build-appimage.sh's "will not be self-inspectable" warning went unread
(run 34641628065). packaging.yml now installs the package; release dry run
34642125623 rebuilt the packages with it, and run 34644096183 confirmed
`discoverer=bundled as a command` on both Linux containers. The check scripts
also no longer assume it: where the command is absent the render's own
--verify probe carries the validation, and the facts say which was used.

AC 1 is deliberately left unchecked: it names macOS, and the macOS half is
deferred with TASK-105/117 (there is no dmg to install). The Ubuntu and
Windows halves are proven by run 34644096183. Whether to narrow AC 1 to the
two shipped platforms and move the macOS half to TASK-117 is the user's call.

2026-09-11 supervisor: criterion 1 narrowed to Linux and Windows (proven by run 34644096183 on clean ubuntu:24.04, fedora:41, hosted ubuntu-26.04 Flatpak and hosted windows-latest MSI); the macOS fresh-install check is added to TASK-117. Note: v0.1.0 was tagged from main before this branch merged, so its AppImage lacks the gst-discoverer-1.0 command; the next tag includes the fix.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fresh-install workflow that installs only the packages on machines that never built the project (Ubuntu and Fedora containers, hosted Ubuntu Flatpak, hosted Windows MSI), opens the sample project, exports and reads it back; found and fixed a missing discoverer tool in the AppImage. All four machines pass in run 34644096183.
<!-- SECTION:FINAL_SUMMARY:END -->
