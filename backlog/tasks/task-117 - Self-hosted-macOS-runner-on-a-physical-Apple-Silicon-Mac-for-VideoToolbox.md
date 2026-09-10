---
id: TASK-117
title: >-
  Self-hosted macOS runner on the user's own Apple Silicon Mac for VideoToolbox
  (no cloud Macs)
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
updated_date: '2026-09-10 00:03'
labels:
  - infra
  - gpu
  - manual
milestone: m-8
dependencies:
  - TASK-116
priority: low
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
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: user ruled out cloud Macs on cost. Blocked until a personal Apple Silicon Mac is available; agents must not provision EC2 mac instances.

2026-09-10: user has bid on an M1 Mac mini (eBay). Back-burnered until it arrives; when it does, register it as a self-hosted runner the same way as box (see docs/DEVELOPMENT.md), label macos-vt, then wire the VideoToolbox job. M1 supports VideoToolbox H.264 and HEVC hardware encode.
<!-- SECTION:NOTES:END -->
