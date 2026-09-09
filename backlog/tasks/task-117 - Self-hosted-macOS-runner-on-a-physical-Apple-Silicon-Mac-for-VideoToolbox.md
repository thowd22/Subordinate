---
id: TASK-117
title: Self-hosted macOS runner on a physical Apple Silicon Mac for VideoToolbox
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
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
Every hosted macOS VM (GitHub arm64 runners, Tart-based providers) exposes Metal but not the VideoToolbox hardware encoder, which silently falls back to software. Hardware VideoToolbox verification needs a physical Mac: the user's own Apple Silicon Mac registered as a self-hosted runner, or an EC2 mac2 dedicated host (24-hour minimum, about 16 to 29 USD per block) for one-off runs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A self-hosted runner labelled macos-vt is registered and online, or an EC2 mac2 procedure is documented and exercised once
- [ ] #2 hardware.yml gains a macOS job that renders with vtenc_h264_hw and confirms hardware acceleration in the log
<!-- AC:END -->
