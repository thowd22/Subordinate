---
id: TASK-158
title: 'Preview playback is dark, silent and out of sync'
status: In Progress
assignee:
  - '@codex'
created_date: '2026-09-13 00:04'
updated_date: '2026-09-13 00:37'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 175000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On v0.1.5 on yodaddy, adding a clip now works, but the viewer is very dark, timeline scrubbing does not move the red playhead, playback leaves timecode static, audio is silent and video appears too fast. Trace the assembled editor paths rather than relying on isolated scheduler or render tests.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Decoded SDR preview preserves source brightness through the actual viewer display path
- [ ] #2 Scrub and playback update the timeline playhead and displayed timecode consistently
- [ ] #3 Normal playback follows elapsed media time at 1x, including when decoders run ahead
- [ ] #4 A dropped video with audio produces audible mixed output; muted and audio-free sources remain correct
- [ ] #5 Routine UI and MCP regression coverage exercises the integrated preview and transport paths
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Investigate viewer texture transfer, scheduler-to-panel synchronization, decode-ahead frame selection and mixer source/output wiring in parallel. Reproduce each failure with meaningful assembled-editor or pipeline regressions, implement narrow fixes, and verify locally then on reusable box/yodaddy runners.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
User confirms SDR 4K60 source about90minutes. Reproduced double sRGB display conversion, scrub mode parking audio clock, overshot decode-ahead consuming future frames, and timeline playhead hit testing. Actual viewer color and assembled transport regressions now pass locally. Adding bounded rolling PCM source windows and shared GUI/MCP transport; no new paid EC2 runs dispatched.

Implemented and locally validated: sRGB display view, red-playhead dragging, scrub-to-play clock release, overshot/EOF decode-ahead hold, shared GUI/MCP transport with project revision guards, paired A/V source drops, and worker-decoded4s PCM windows with64MiB cache+pending budget. Long-source regression seeks near89:58 of90min60fps sources without full decode. Exact audio seek/EOF tests pass. Final local checks:637 UI/audio/edit unit tests, all5 focused UI test binaries,17 audio-window/decode tests,3 native harness parser tests, strict Clippy across6 changed crates. Box/yodaddy packaging now includes playback tests; native flow project-envelope parsing corrected from release logs. Remote regression verification pending.

Final review added explicit retry for GUI transport mailbox contention/newer revisions so a paused MCP seek cannot lose its only wakeup.119 edit unit tests and4 assembled transport tests pass; targeted strict Clippy passes. Superseded free CI/regression runs canceled before restarting at final revision.
<!-- SECTION:NOTES:END -->
