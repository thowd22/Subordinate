---
id: TASK-117
title: >-
  Self-hosted macOS runner on the user's own Apple Silicon Mac for VideoToolbox
  (no cloud Macs)
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
updated_date: '2026-09-12 04:07'
labels:
  - infra
  - gpu
  - manual
milestone: m-8
dependencies:
  - TASK-116
priority: medium
ordinal: 137000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every hosted macOS VM (GitHub arm64 runners, Tart-based providers) exposes Metal but not the VideoToolbox hardware encoder, which silently falls back to software. Hardware VideoToolbox verification therefore needs a physical Mac. Decision 2026-09-09: do NOT use EC2 mac2 dedicated hosts or any paid cloud Mac (24-hour minimum billing makes them expensive); the only acceptable path is the user's own Apple Silicon Mac registered as a self-hosted GitHub runner, if and when one is available. Until then VideoToolbox stays verified by the software path on macos-latest only.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A self-hosted runner labelled macos-vt is registered and online, or an EC2 mac2 procedure is documented and exercised once
- [ ] #2 hardware.yml gains a macOS job that renders with vtenc_h264_hw and confirms hardware acceleration in the log
- [ ] #3 A self-hosted runner labelled macos-vt on a user-owned Apple Silicon Mac is registered and online
- [ ] #4 hardware.yml gains a macOS job gated on that label that renders with vtenc_h264_hw and confirms hardware acceleration in the log
- [ ] #5 Fresh-install check on the Mac: install the dmg, open the sample project, play with audio, export with vtenc_h264_hw (moved from TASK-110)
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: user ruled out cloud Macs on cost. Blocked until a personal Apple Silicon Mac is available; agents must not provision EC2 mac instances.

2026-09-10: user has bid on an M1 Mac mini (eBay). Back-burnered until it arrives; when it does, register it as a self-hosted runner the same way as box (see docs/DEVELOPMENT.md), label macos-vt, then wire the VideoToolbox job. M1 supports VideoToolbox H.264 and HEVC hardware encode.

2026-09-12: the user bought the M1 Mac mini (about 400 USD). When it arrives: install Homebrew, GStreamer 1.28 (brew), Rust 1.93.1, register a self-hosted runner labelled self-hosted,macos,arm64,macmini,videotoolbox as a launchd agent in the user's session (same shape as box and yodaddy), verify vtenc_h264_hw with a hardware encode round-trip, then add the VideoToolbox job to hardware.yml and the macOS jobs to export-matrix.yml and the desktop flows; TASK-105 (dmg) and TASK-66's VideoToolbox half unblock after that.
<!-- SECTION:NOTES:END -->
