---
id: TASK-123
title: Xvfb window smoke test with screenshots on hosted Linux CI
status: To Do
assignee: []
created_date: '2026-09-09 18:21'
labels:
  - ui
  - test
  - infra
milestone: m-2
dependencies:
  - TASK-43
  - TASK-67
priority: medium
ordinal: 143000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
kittest exercises panels in isolation; this exercises the real assembled app window and its pop-out viewport under a virtual X display on the free ubuntu runner. Screenshots are uploaded so agents can inspect the actual app visually without any GPU cost.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A CI job on ubuntu-26.04 starts Xvfb with two screens, launches subordinate with the sample project and a --ui-smoke flag that opens the pop-out viewer on the second screen, waits for first frame, and captures a PNG of each screen
- [ ] #2 Screenshots and the app log are uploaded as artifacts named ui-smoke-<sha>; a job summary lists them with dimensions so an agent can gh run download and read them
- [ ] #3 Job adds under 90 seconds to the Linux CI run and is skipped on Windows and macOS
<!-- AC:END -->
