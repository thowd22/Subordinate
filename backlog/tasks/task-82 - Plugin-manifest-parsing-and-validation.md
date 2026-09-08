---
id: TASK-82
title: Plugin manifest parsing and validation
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
references:
  - docs/PLAN.md
priority: high
ordinal: 103000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The manifest (§6.3) declares identity, worlds and capabilities.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 plugin.toml parsed into a Manifest struct: id (reverse-DNS), name, version (semver), api version, worlds, capabilities, mcp tool declarations
- [ ] #2 Validation errors are SubError with field paths
- [ ] #3 A JSON Schema for the manifest is generated and committed
<!-- AC:END -->
