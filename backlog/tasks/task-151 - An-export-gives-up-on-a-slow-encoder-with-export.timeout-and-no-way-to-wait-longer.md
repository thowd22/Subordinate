---
id: TASK-151
title: >-
  An export gives up on a slow encoder with export.timeout and no way to wait
  longer
status: To Do
assignee: []
created_date: '2026-09-12 12:00'
labels:
  - export
  - bug
dependencies: []
ordinal: 171000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The export matrix (TASK-143) renders eight frames of the sample project with av1enc, libaom AV1 encoder. On a free hosted four-core runner that takes 82 seconds and passes; on the T4 instance, which is also four cores but busier, the same eight frames end in "export.timeout: the export pipeline never finished writing" (run 34689001087) and no file. Nothing is wrong with the encoder or the file it would have written - it is simply slower than the pipeline fixed patience, and libaom at stock cpu-used is seconds to minutes a frame at any resolution a user would actually export.

So an export that would have succeeded is abandoned, and the user is told the pipeline never finished writing rather than that it ran out of time. Two things are missing: the timeout should be about progress rather than about total elapsed time, since an encoder that is producing a frame a minute is working; and where a hard limit is still wanted it belongs in the export request rather than being a constant.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An export that is still making progress is not abandoned, however slow the encoder is
- [ ] #2 An export that has genuinely stopped is failed with an error that says so and names the element it was waiting on
- [ ] #3 A test covers a deliberately slow encoder finishing an export that the current fixed timeout would abandon
<!-- AC:END -->
