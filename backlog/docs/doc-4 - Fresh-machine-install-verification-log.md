---
id: doc-4
title: Fresh-machine install verification log
type: other
created_date: '2026-09-11 20:27'
updated_date: '2026-09-11 20:29'
---
Every run of `.github/workflows/fresh-install.yml` (TASK-110) is recorded here:
the machine, its OS version, its GPU, the encoder the package chose and the
result. A run by hand on a physical machine goes in the same table — see
docs/DEVELOPMENT.md, "Fresh-machine install verification", for the runbook.

A failure in any row becomes a blocking task before the release it was checking
goes out.

## What a row proves

The machine had never built this project: no Rust toolchain, no GStreamer
development files, and on Windows no `C:\gstreamer` and nothing GStreamer on
`PATH` (the check asserts all of that before it installs anything). It
downloaded only the package, installed it, opened
`examples/sample-project/demo.sub` with every media path resolved and nothing
offline, probed the media, exported one second of "Main cut" with the best
encoder the bundled runtime could actually use, read the result back, and
opened the project in the real editor window.

## Runs

### 2026-09-11 — packages from release dry run 34642125623

Fresh-install run 34644096183 — all four machines passed.

| Machine | OS | GPU | Package | GStreamer | Encoder used | Export | UI | Result |
|---|---|---|---|---|---|---|---|---|
| clean `ubuntu:24.04` container | Ubuntu 24.04.4 LTS (glibc 2.39) | none (no `/dev/dri`) | AppImage | 1.24.2 bundled | `x264enc` | 162109 B, H.264 + FLAC | opened, popped out | pass |
| clean `fedora:41` container | Fedora Linux 41 (glibc 2.40) | none (no `/dev/dri`) | AppImage | 1.24.2 bundled | `x264enc` | 161603 B | opened, popped out | pass |
| hosted `ubuntu-26.04` runner, no flatpak until the job installed one | Ubuntu 26.04.1 LTS (glibc 2.43) | none usable (llvmpipe) | Flatpak bundle | 1.26.11 from the freedesktop runtime | `x264enc` | 174370 B | opened, popped out | pass |
| hosted `windows-latest` runner, no GStreamer anywhere | Windows Server 2025 Datacenter 10.0.26100 | Microsoft Hyper-V Video | MSI | 1.28.6 bundled | `mfh264enc` | 90856 B | opened, popped out | pass |

Notes:

- The Windows machine chose `mfh264enc` over `x264enc`: Media Foundation's
  H.264 transform is part of Windows and outranks the software encoder in
  `sub_export`'s order, and with no GPU behind it, it encodes in software.
  `nvh264enc` and `amfh264enc` were absent, as they are on a machine with no
  driver to register them.
- Every machine in this table is GPU-less, so "best available encoder" resolves
  to a software one. Hardware encode out of the packages is `hardware.yml`'s
  question and `packaging.yml`'s `packages-amd` job, and the optional
  `hardware=true` jobs here.

### 2026-09-11 — earlier attempts, and what they found

| Run | Result | What it found |
|---|---|---|
| 34641476353 | fail | The check script was being fetched at the *packages'* commit, where it does not exist. The project now comes from the packages' commit and the harness from the workflow's own. |
| 34641628065 | fail (AppImage only) | **A real packaging gap.** The AppImage carried the GStreamer discoverer *library* but not the `gst-discoverer-1.0` *command*: on Ubuntu it lives in `gstreamer1.0-plugins-base-apps`, which the build container never installed, and `build-appimage.sh`'s "the package will not be self-inspectable" line went unread. `packaging.yml` now installs it. The Flatpak and the MSI passed this run, editor window included. |
| 34642119698 | pass | All four machines, against the packages from release dry run 34638014639 — the AppImage *without* the discoverer, its export validated by the render's own in-process `--verify` probe instead. |

## Not covered yet

- **macOS.** Deferred with TASK-105 (the dmg) and TASK-117. There is nothing to
  install on a fresh Mac until a package exists; `scripts/fresh-install-check.sh`
  already handles Darwin, so the third leg is a job and an adapter.
- **A physical fresh machine.** Every row above is a hosted runner or a
  container on one. The runbook in docs/DEVELOPMENT.md is for a real laptop with
  a real GPU and a real desktop session; add a row when one is run.
- **The FUSE path.** Containers have no `/dev/fuse`, so CI uses the AppImage
  runtime's own `--appimage-extract`. Double-clicking the file is what the
  by-hand runbook exercises.
