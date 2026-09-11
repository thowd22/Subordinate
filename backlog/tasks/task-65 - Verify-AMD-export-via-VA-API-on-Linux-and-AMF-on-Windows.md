---
id: TASK-65
title: Verify AMD export via VA-API on Linux and AMF on Windows
status: Done
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 20:43'
labels:
  - export
  - verify
milestone: m-4
dependencies:
  - TASK-116
references:
  - docs/PLAN.md
priority: medium
ordinal: 86000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
AMD hardware paths differ per OS and are less exercised than NVENC.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Findings and driver versions recorded in a backlog doc
- [x] #2 VA-API render on AMD Linux verified by the hardware workflow's box job (vah264enc)
- [x] #3 AMF render on AMD Windows verified once AMD Windows hardware exists (none available; no cloud host)
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.

2026-09-09: verify via the hardware workflow on RunsOn GPU runners (and the physical Mac runner for VideoToolbox) rather than by hand.

2026-09-09: AWS no longer offers any AMD GPU instance type (g4ad retired; verified via describe-instance-type-offerings across all regions). AMD parts of this task need Azure NVads V710 v5 or a user-owned AMD box; NVIDIA parts proceed on RunsOn g4dn.

2026-09-09: VA-API half verifiable on box via the hardware workflow; AMF half blocked on AMD Windows hardware.

2026-09-11 supervisor: VA-API on the box APU verified by hardware workflow run https://github.com/thowd22/Subordinate/actions/runs/34612380072 (vah264enc render validated by discoverer; findings recorded in the job summary and DEVELOPMENT.md). AMF half blocked on hardware.

2026-09-11 supervisor verification: GPU smoke run 34643890462, job 'AMD RX 9070 XT (yodaddy, self-hosted, Windows 11)': GStreamer 1.28.6 per-user, amfh264enc/amfh265enc/amfav1enc present; amfh264enc encoded 120 frames of 1080p to a 10.9 MB MP4 that gst-discoverer reports as H.264 Main Profile, 4.000 s. AMF Windows verified on the user's own machine.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
AMD export verified on both platforms: VA-API (vah264enc) on the box APU via the hardware workflow, and AMF (amfh264enc) on the user's Windows desktop (RX 9070 XT) via the GPU smoke workflow; findings and driver versions recorded in the job summaries and docs.
<!-- SECTION:FINAL_SUMMARY:END -->
