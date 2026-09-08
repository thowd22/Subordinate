---
id: TASK-10
title: Test media fixture generator script
status: To Do
assignee: []
created_date: '2026-09-08 21:04'
labels:
  - infra
  - media
milestone: m-0
dependencies:
  - TASK-1.3
references:
  - docs/PLAN.md
priority: high
ordinal: 14000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Media tasks need deterministic sample files but binaries must not be committed. A script that synthesises them with gst-launch gives every developer and CI runner identical inputs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/gen-fixtures.sh (and a PowerShell equivalent or cross-platform Rust xtask) generates: 1080p and 4K H.264 colour bars with timecode burn-in, a 29.97 drop-frame clip, a variable-frame-rate clip, a 10-minute long-GOP clip, a WAV and a FLAC audio-only file
- [ ] #2 Generated files land in fixtures/ which is gitignored, and a manifest JSON records name, duration, fps, VFR flag
- [ ] #3 CI generates the small fixtures (not the 10-minute clip) before running tests
- [ ] #4 Tests can locate fixtures via a helper in a shared test-support crate
<!-- AC:END -->
