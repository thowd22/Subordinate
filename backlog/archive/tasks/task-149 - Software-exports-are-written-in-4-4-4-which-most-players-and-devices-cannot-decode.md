---
id: TASK-149
title: >-
  Software exports are written in 4:4:4, which most players and devices cannot
  decode
status: To Do
assignee: []
created_date: '2026-09-12 08:10'
labels:
  - export
  - bug
dependencies: []
ordinal: 169000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The export matrix (TASK-143) read back every file it wrote, and every x264enc cell came out as "H.264 (High 4:4:4 Profile)" - run 34672568992, hosted ubuntu-24.04. The pipeline is appsrc(RGBA) ! videoconvert ! encoder with no format pinned on the encoder side, so videoconvert negotiates the least-lossy conversion x264enc will take, which is Y444. The result is a YouTube 1080p export in a profile YouTube itself re-encodes, browsers and phones refuse, and no hardware decoder on any of this project four test machines can play. It is invisible from inside the app: the file exists, the frame count is right and the picture is correct, and only a probe of the finished file says what profile it is in. The hardware encoders hide the problem rather than sharing it - VA-API and NVENC take NV12 and nothing else - so the software path is the one that ships a file a user cannot use.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The export pipeline pins the encoder input to 4:2:0 (I420 or NV12 as the element prefers) so a default export is High or Main profile, not High 4:4:4
- [ ] #2 A preset may still ask for a higher chroma format where the codec and the element support it, and an element that cannot take what was asked for fails with a named error rather than silently negotiating something else
- [ ] #3 A test asserts the profile of a written file for at least the software H.264 and H.265 encoders
<!-- AC:END -->
