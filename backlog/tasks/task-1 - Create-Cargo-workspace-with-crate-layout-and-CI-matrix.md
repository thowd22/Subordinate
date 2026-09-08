---
id: TASK-1
title: Create Cargo workspace with crate layout and CI matrix
status: In Progress
assignee:
  - '@claude'
created_date: '2026-09-08 20:53'
updated_date: '2026-09-08 21:23'
labels:
  - infra
milestone: m-0
dependencies: []
references:
  - docs/PLAN.md
priority: high
ordinal: 1000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The repo is empty. The plan (docs/PLAN.md §4) defines a layered workspace: sub-time, sub-model, sub-edit, sub-media, sub-audio, sub-render, sub-export, sub-command, sub-plugin, sub-ui crates plus subordinate, subordinate-cli and subordinate-mcp binaries. CI must install a pinned GStreamer 1.28 runtime on each OS from day one because bundling GStreamer is the top packaging risk.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Workspace builds with cargo build on Linux, Windows and macOS in GitHub Actions
- [x] #2 Every crate from PLAN.md §4 exists with a lib.rs and a one-line doc comment
- [ ] #3 CI installs a pinned GStreamer version on all three runners and gst-inspect succeeds
- [x] #4 Licence files present: GPL-3.0-or-later at root, MIT and Apache-2.0 under sdk/ and wit/
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Subtasks 1.1 and 1.4 Done. 1.2 and 1.3 are fully authored (ci.yml, actionlint clean, DEVELOPMENT.md, sub-media depends on gstreamer 0.25 with an init test) and verified locally on Linux, including a GStreamer link test against an extracted-deb prefix. Criteria 1 and 3 here, plus the remaining criteria on 1.2 and 1.3, require an actual GitHub Actions run; the repository has no remote and no commits yet, so those stay unchecked until the user creates the remote (proposed: github.com/thowd22/Subordinate, matching the workspace repository field) and the work is committed and pushed.
<!-- SECTION:NOTES:END -->
