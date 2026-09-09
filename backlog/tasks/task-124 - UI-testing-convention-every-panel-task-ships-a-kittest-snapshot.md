---
id: TASK-124
title: 'UI testing convention: every panel task ships a kittest snapshot'
status: To Do
assignee: []
created_date: '2026-09-09 18:21'
labels:
  - ui
  - test
  - docs
milestone: m-2
dependencies:
  - TASK-119
priority: medium
ordinal: 144000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Without a convention, new panels will land untested again. Encode the rule where agents read it and make the harness easy enough that the rule is cheap to follow.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 CLAUDE.md conventions list states that any task touching sub-ui adds or updates a kittest snapshot or interaction test, and names the harness module
- [ ] #2 docs/DEVELOPMENT.md has a UI testing section covering the harness, snapshot update, artifact inspection for agents, and the rule that GPU runners are only for GPU-specific checks
- [ ] #3 Open UI tasks in m-2 and m-5 (inspector, crossfade UI, markers, docking, pop-out, fullscreen, effect UI, plugin panel, export panel) have a snapshot acceptance criterion appended
<!-- AC:END -->
