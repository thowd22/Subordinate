---
id: TASK-135
title: >-
  Feed the compositor readback and offline audio mix into the export job so the
  app's Export button works
status: To Do
assignee: []
created_date: '2026-09-11 19:07'
labels:
  - export
  - ui
  - render
  - audio
milestone: m-4
dependencies:
  - TASK-62
  - TASK-63
references:
  - docs/PLAN.md
priority: high
ordinal: 155000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-62 wired the export panel to sub_export's job (progress, ETA, cancel) through an ExportSources trait, but app::export_sources() still returns core.unimplemented and the app's Export button stays disabled with app::NO_RENDERER_REASON. The pieces exist separately: sub-render's full-resolution readback with a staging ring (TASK-58), sub-audio's offline render through the mixer (TASK-55), and subordinate-cli's render path that already drives both (TASK-63, bins/subordinate-cli/src/render.rs). The GUI is the only client that cannot export, which blocks the MVP. Reuse the CLI's frame and audio source adapters rather than writing new ones; the app owns a wgpu device and an engine handle already.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 app::export_sources() returns real VideoFrameSource and AudioFrameSource implementations backed by the compositor readback and the offline mix for the active sequence, and the Export button is enabled when a project is open
- [ ] #2 Exporting the sample project from the assembled app produces a file the discoverer validates, with frame count equal to the sequence length at the preset rate and audio present; proven by a headless kittest/Xvfb test on the software adapter and recorded in the Linux CI job
- [ ] #3 Export from the GUI and from subordinate-cli render of the same project and preset produce byte-identical audio and identical video frame counts
<!-- AC:END -->
