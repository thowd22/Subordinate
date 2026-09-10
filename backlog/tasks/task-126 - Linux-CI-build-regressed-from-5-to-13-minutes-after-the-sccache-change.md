---
id: TASK-126
title: Linux CI build regressed from 5 to 13 minutes after the sccache change
status: To Do
assignee: []
created_date: '2026-09-10 00:47'
labels:
  - infra
  - ci
milestone: m-0
dependencies: []
references:
  - .github/workflows/ci.yml
priority: medium
ordinal: 146000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-125 fixed Windows CI (36m to 8m) but a warm-cache no-change rerun of run 34419018398 shows ubuntu-26.04 taking 13m with a 9m Build step and sccache reporting 164 hits versus 167 misses, where the same job took about 5m before sccache and line-tables debuginfo were introduced. Something on Linux is recompiling roughly half the crate graph on every run: suspects are the Swatinem cache and sccache interacting badly (both caching the same artifacts, restore of one invalidating the other), a fingerprint difference between the Build, Test and Clippy steps, or the fixture-generation and GUI smoke steps mutating inputs. Measure per-step and read sccache --show-stats before changing anything.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 ubuntu-26.04 job on a warm no-change rerun completes in under 7 minutes, recorded in the task notes with the run id
- [ ] #2 sccache hit rate on Linux is above 90 percent on the warm rerun, or sccache is disabled on Linux with the reason documented in ci.yml
- [ ] #3 Windows stays under 15 minutes on the same rerun
<!-- AC:END -->
