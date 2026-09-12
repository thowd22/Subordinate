---
id: TASK-157
title: Import and edit media before the first save
status: In Progress
assignee:
  - codex
created_date: '2026-09-12 21:44'
updated_date: '2026-09-12 22:08'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 174000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fresh projects currently refuse import until saved and only accept media beneath the project folder. User expects to begin editing immediately without copying large source files. First save and in-flight imports must retain source locations and undo history, including Windows sources on another drive.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An unsaved editor imports and previews media from arbitrary folders without copying sources
- [ ] #2 First Save As, reopen, and import undo/redo preserve source references
- [ ] #3 Routine UI and MCP checks cover fresh-project import, track creation and save/reopen
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Add explicit external media references alongside strict relative paths; use stable session import/cache context; cover pointer and MCP first-use workflows with real fixtures across routine runners.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added explicit tagged external media references alongside legacy strict relative strings. GUI imports retain stable source paths before first Save and through in-flight imports, preview, Undo/Redo and reopening. Real fixture-based assembled-editor regression and real MCP subprocess regression pass. Combined model, Command API, UI and MCP suites passed 955 tests with no failures (one existing ignored test). Routine desktop flows now begin with unsaved projects rather than staged populated projects; same-commit artifact provisioning and box/yodaddy offscreen regression jobs are being integrated.

Routine coverage is wired into normal workspace UI/MCP tests and Desktop flows for Linux/Windows GPU runners plus box/yodaddy. Hosted builders package the selected commit with SHA/hash verification; focused regressions generate their own fixtures, reject skipped tests, and isolate configuration and MCP endpoints. All four relocated regressions passed locally. Remote runner validation pending.
<!-- SECTION:NOTES:END -->
