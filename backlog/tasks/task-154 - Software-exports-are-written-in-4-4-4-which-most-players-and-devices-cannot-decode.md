---
id: TASK-154
title: >-
  Software exports are written in 4:4:4, which most players and devices cannot
  decode
status: In Progress
assignee:
  - '@codex'
created_date: '2026-09-12 04:40'
updated_date: '2026-09-12 17:55'
labels:
  - export
  - bug
dependencies: []
ordinal: 166000
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a ChromaFormat type (4:2:0 default, 4:2:2, 4:4:4) in a new sub-export chroma module: stable id, label, and the raw video/x-raw formats of each family.
2. Carry it on ExportSettings (with_chroma, default 4:2:0) and on VideoPreset via a new optional 'chroma' preset field, plumbed through Preset::to_settings and settings_for_sequence.
3. Pin the encoder input in build_video_branch: appsrc ! videoconvert ! capsfilter(video/x-raw, format={family formats the encoder's sink template declares, in the element's own order}) ! encoder, so a default export is 4:2:0 whatever videoconvert would otherwise negotiate.
4. Fail loudly when the element declares no format of the requested family: new stable code export.chroma_unsupported naming the element, the chroma asked for and what it does take.
5. Tests: unit tests for the format-selection helper against synthesised sink caps (x264-like, NV12-only, 4:4:4 refusal), preset parse/round-trip tests, and an integration test that exports with x264enc and x265enc and asserts the profile read back off the file is 4:2:0 (and that an explicit 4:4:4 preset still writes 4:4:4).
6. Verify with cargo fmt, clippy -D warnings and the sub-export tests under the local GStreamer env.

Resume preserved agent implementation; integrate with TASK-146 and the other export fixes on a shared export branch, then validate locally without AWS.
<!-- SECTION:PLAN:END -->
