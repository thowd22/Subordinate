---
id: TASK-153
title: A source with more than one audio stream exports only its first
status: Done
assignee:
  - '@codex'
created_date: '2026-09-12 04:33'
updated_date: '2026-09-12 18:09'
labels:
  - audio
  - export
  - bug
dependencies: []
ordinal: 165000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user's 4K60 test footage carries three 48 kHz stereo AAC tracks, and the export matrix (TASK-143) shows every export of it - every encoder, every preset, both the CLI and the GUI - writing one stereo track. uridecodebin exposes one audio pad per stream only when it is asked to; sub_media::decode opens a single audio branch, so the second and third tracks are dropped without a warning anywhere: not in the media bin, not in the render report, not in the export warnings. A multi-track camera master or a mix-minus feed is a normal thing to cut, and silently keeping only track one is the kind of loss a user finds after delivery.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The media probe reports every audio stream a file carries, and the bin and the inspector show how many there are
- [x] #2 A clip names which of the source's audio streams it takes, defaulting to the first, and the decoder opens that one
- [x] #3 An export whose source carries streams the project does not use says so in its warnings rather than dropping them silently
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Preserve and review inherited multi-stream implementation; add a backward-compatible u16 clip audio-stream selection and undoable validated command.
2. Decode the selected source stream in GStreamer and symphonia without changing source stream positions.
3. Show source stream counts in the bin and inspector, with an undoable picker for editable clips.
4. Report unused audible streams in the panel, CLI reports, and GUI export worker; probe unknown export sources once per path and report probe failures.
5. Verify model defaults/round trips, command undo/rejection, real-file probing and decoding, unprobed warnings, UI interactions and reviewed snapshots. Regenerate both schemas and run focused tests plus strict Clippy.
6. Hand off to root for integration with latest main/TASK-150 routing.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Resumed preserved agent changes. Updated inspector test call sites for the project-aware API; added an egui picker/undo interaction test, media-bin count rendering check, multi-stream GStreamer probe/decode regression, export-warning regression, and command exact-undo/out-of-range coverage. Warning accounting now excludes inaudible muted/non-solo/video tracks. Initial native build lacked pkg-config environment; resumed with /home/admin2/.cache/subordinate/env-gst.sh. Model/edit/audio library tests: 328 passed.

Inspector and bin interaction/render suites pass (17 and 15 tests). Snapshots regenerated and visually reviewed; Audio column moved next to Name so the stream count is visible in a narrow bin. Edit command integration suite: 28 passed. Native audio decode suite: 10 reported passing; existing no-audio fixture test skipped because video fixture is absent, while the new multi-stream probe/decode test executed successfully. Standalone Command API socket test fails under sandbox and passes outside it; no socket implementation changes made.

Export-time warnings now probe referenced unprobed sources once per original path, disable frame timing scans, leave the project untouched, and explicitly warn when probing fails. CLI JSON warnings and GUI export-worker logs use this path. Verified with 68 export unit tests plus 2 new real-file/failed-probe integration tests; all pass. Muted/video-only selections no longer hide unused streams.

Final verification: strict cargo clippy across sub-model, sub-edit, sub-audio, sub-media, sub-export and sub-ui with --all-targets -- -D warnings passes. Both generated project/Command API schemas are refreshed; three schema freshness tests pass without update mode. Rechecked 114 audio unit tests, real multi-stream decoder selection regression, and 17 inspector plus 15 media-bin tests after final changes; all pass. Environment: source /home/admin2/.cache/subordinate/env-gst.sh; reusable build cache is this worktree target directory. No AWS or remote runner usage. Root will integrate TASK-150 already merged on main; preserve the audio_stream argument when reconciling decode_audio.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Clips now select a source audio stream, defaulting to the first, with undoable inspector editing and decoder routing. Bin/inspector expose counts, and exports warn about unused streams including unprobed sources or probe failures. Verified real multi-stream fixtures, command/model round trips, UI interactions and reviewed snapshots, schema freshness, and strict Clippy. Integrated-main routing verification remains with root.
<!-- SECTION:FINAL_SUMMARY:END -->
