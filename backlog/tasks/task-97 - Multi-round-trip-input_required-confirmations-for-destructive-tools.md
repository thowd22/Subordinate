---
id: TASK-97
title: Multi round-trip input_required confirmations for destructive tools
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
labels:
  - mcp
milestone: m-6
dependencies:
  - TASK-94
references:
  - docs/PLAN.md
priority: medium
ordinal: 118000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Deleting sequences or overwriting files should ask before proceeding (§7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Destructive tools (delete_sequence, remove_media, export overwrite) return input_required unless a confirm flag is set
- [ ] #2 Claude Code's elicitation dialog is verified manually
- [ ] #3 Non-interactive clients can pass confirm=true
<!-- AC:END -->
