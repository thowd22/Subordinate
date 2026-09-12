---
id: TASK-156
title: Imported media cannot be dropped onto a fresh empty timeline
status: Done
assignee:
  - codex
created_date: '2026-09-12 21:31'
updated_date: '2026-09-12 21:36'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 173000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
User installed v0.1.4 on yodaddy and can import a video but cannot drag it onto the timeline. Fresh app starts with no project sequence/tracks while displaying a fallback timeline; source edits currently require an existing lane. Investigate actual pointer behavior as well as empty-project initialization.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Imported video can be dragged onto a fresh timeline to create an editable clip
- [x] #2 The edit is undoable and existing sequence drops retain their behavior
- [x] #3 An interaction regression covers the fresh project path
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Reproduce empty-project and pointer drag paths, implement the smallest coherent timeline bootstrap, and validate with real pointer interaction plus existing bin edit tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
User confirmed the floating clip label follows the pointer but dropping creates no track. Assembled-editor pointer regression reproduces the fresh-project failure before changes: zero sequences after drop. Scope includes no sequence and an existing sequence with zero tracks, with one-step undo and redo preserving imported media and edit identities.

Implemented gesture-only bootstrap using InsertSequence/InsertTrack and clip placement in one command group. Existing populated tracks keep prior refusal rules. Real assembled-window pointer regression failed before and passes after; it covers absent and existing-empty sequence, Undo and Redo. Six existing bin-edit interaction/snapshot tests and six source-edit unit tests pass. All-target sub-ui clippy with warnings denied and workspace formatting pass. No EC2 jobs used.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
An imported clip dropped onto an empty timeline now creates the first appropriate track and, when needed, a real sequence. Bootstrap and clip placement undo/redo as one edit. Fixed both the no-track planner refusal and the application early return when no sequence tab exists. Verified through an actual pointer gesture in the assembled editor and existing edit regressions.
<!-- SECTION:FINAL_SUMMARY:END -->
