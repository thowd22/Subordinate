---
id: TASK-136
title: >-
  Import and relink from the media bin do nothing in the app: host the import
  and relink jobs in SubordinateApp
status: To Do
assignee: []
created_date: '2026-09-11 21:01'
labels:
  - ui
  - media
  - bug
milestone: m-2
dependencies:
  - TASK-127
  - TASK-35
priority: high
ordinal: 156000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
User report on the v0.1.0 MSI: choosing Import in the media bin logs 'the bin asked for work this window does not host yet: Import { paths: [...mkv], bin: BinId(...) }' and nothing happens. SubordinateApp::apply_bin_action only applies actions that map to a single command; Import and Relink are jobs (content hash, probe via sub-media, then ImportMedia/RelinkMedia commands, then thumbnail and waveform jobs) and the window has no host for them (crates/sub-ui/src/app.rs, apply_bin_action). The CLI and MCP paths that probe media exist (TASK-94's Services in bins/subordinate-cli/src/host.rs; sub-media probe and hashing; the job service from TASK-25). The GUI must run the same pipeline off the UI thread, show progress in the bin, apply the resulting commands through the session (one undo entry per import batch), and kick off thumbnails and waveforms. Drag-and-drop from the OS onto the bin must go through the same path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Import from the bin's Import button, from a native file dialog selection and from an OS drag-and-drop probes the files off the UI thread, adds MediaItems with hash and stream info through the Command API as one undo step, and starts thumbnail and waveform jobs; the bin shows the items with a pending state until probing finishes
- [ ] #2 Relink from the bin runs the relink search as a job and applies RelinkMedia; offline items clear their badge
- [ ] #3 An interaction test on the assembled app imports a generated fixture (and an MKV, since the report was an MKV) and asserts the media item appears with correct duration; an unreadable path surfaces a SubError in the bin rather than a log line
- [ ] #4 No remaining 'does not host yet' branch in app.rs; every MediaBinAction is handled
<!-- AC:END -->
