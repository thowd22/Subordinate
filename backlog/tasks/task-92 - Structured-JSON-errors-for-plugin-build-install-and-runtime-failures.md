---
id: TASK-92
title: 'Structured JSON errors for plugin build, install and runtime failures'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-86
references:
  - docs/PLAN.md
priority: medium
ordinal: 113000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Errors are the agent's main feedback signal (§6.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 All plugin failures surface as SubError JSON with code, message, WIT type or function name, and a hint
- [ ] #2 Manifest, capability and reload errors have distinct codes documented in the SDK
- [ ] #3 CLI prints them as JSON with --json
<!-- AC:END -->
