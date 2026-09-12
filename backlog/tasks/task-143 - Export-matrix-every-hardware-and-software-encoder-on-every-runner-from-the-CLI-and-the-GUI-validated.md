---
id: TASK-143
title: >-
  Export matrix: every hardware and software encoder on every runner, from the
  CLI and the GUI, validated
status: To Do
assignee: []
created_date: '2026-09-12 03:58'
labels:
  - export
  - gpu
  - test
milestone: m-4
dependencies:
  - TASK-135
  - TASK-116
  - TASK-138
priority: high
ordinal: 163000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Export has been verified one encoder at a time (hardware workflow: nvh264enc on the T4, vah264enc on box; AMF smoke on the user's desktop; x264 in CI) but never as a matrix, never through the GUI export path on hardware (TASK-135's note), and never for HEVC or AV1. Build a matrix job set that renders the sample project and the user's 4K60 three-audio-track excerpt with every encoder each machine exposes, from subordinate-cli and through the assembled app's export runner, with each preset, validating stream layout, frame count, duration and audio presence with gst-discoverer, and comparing GUI versus CLI outputs. Machines: box (vah264enc, vah265enc, x264enc, x265enc), NVIDIA Linux runner (nvh264enc, nvh265enc, nvav1enc), NVIDIA Windows runner (nvh264enc, nvh265enc, mfh264enc, mfh265enc), the user's Windows desktop yodaddy (amfh264enc, amfh265enc, amfav1enc), hosted runners (software). Failures must name encoder, preset, machine and the GStreamer error. Keep GPU jobs on spot and short; never run more than two g4dn jobs at once.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 hardware.yml (or export-matrix.yml called from it) runs the matrix on demand and nightly; the job summary is a table of machine x encoder x preset x source with pass/fail and file size, and artifacts hold every failing output and its discoverer report
- [ ] #2 Every encoder that the machine's gst-inspect reports present either passes or has a filed bug task with the error; the 4K60 excerpt exports with hardware encoders on box, the T4 and yodaddy
- [ ] #3 GUI-path exports (sub-ui export runner on the Linux desktop image) match CLI exports on frame count and audio for at least nvh264enc and x264enc
<!-- AC:END -->
