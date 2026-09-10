---
id: TASK-44
title: >-
  End-to-end test: assemble a 20-clip edit with cuts and crossfades through the
  Command API
status: Done
assignee:
  - '@opus-task-44'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 21:56'
labels:
  - test
milestone: m-2
dependencies:
  - TASK-38
  - TASK-39
  - TASK-5.4
references:
  - docs/PLAN.md
priority: medium
ordinal: 65000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Phase 2 exit criterion, automated so it stays true.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A headless test builds a 20-clip, 3-track sequence with splits, trims and crossfades via the Command API
- [x] #2 Undo all then redo all yields identical project JSON
- [x] #3 Rendering frame 100 produces a non-black image
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a headless end-to-end test crates/sub-command/tests/end_to_end_edit.rs driving a real Command API server (Endpoint + transport::Server) over the local socket with a transport::Client, so the whole JSON-RPC path is exercised.
2. Build the edit only through JSON-RPC calls: media.import x3, sequence.create, track.add x3 (V1/V2/V3), clip.add x15, clip.split x5 with deterministic tail ids (20 clips total), clip.trim_in/clip.trim_out, transition.add crossfades including one spanning frame 100.
3. AC2: read project.get, then edit.undo until history.get says can_undo is false, assert the project is back to empty, then edit.redo to the top and assert the project JSON is byte-identical to the snapshot taken before undoing.
4. AC3: deserialise the project from project.get, take the sequence, build a sub-render Compositor on a headless RenderContext, feed a solid-colour SourceFrame per clip, render frame 100 and assert the read-back image is not black (and that the crossfade blended two layers). Skip with a message only when the machine has no wgpu adapter, as the other GPU tests do.
5. Add dev-dependencies to sub-command (sub-render, sub-time, wgpu) and verify with cargo fmt, clippy -D warnings and cargo test -p sub-command.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
New headless test crates/sub-command/tests/end_to_end_edit.rs. It runs a real Command API server (Engine + Dispatcher + transport::Server on a private endpoint) and a transport::Client, so every mutation is a JSON-RPC call over the socket; the command structs are only used to build the params, and go over the wire as their own serde form.

The edit: 3 media.import, 1 sequence.create (64x36 canvas at 24 fps so a software adapter reads a frame back in milliseconds), 3 track.add (V1/V2/V3), 15 clip.add in butt-joined runs, 5 clip.split with explicit tail ids, clip.trim_out and clip.trim_in, 3 transition.add. 32 commands, 32 revisions, 20 clips, 3 crossfades. Only V1 covers frame 100, and the first crossfade is placed on the cut the split at frame 100 made, so the playhead sits exactly halfway through it.

AC1 (a_twenty_clip_edit_is_assembled_through_the_command_api): asserts 3 tracks, 20 clips, 3 transitions, and that resolve_layers_at(frame 100) yields the outgoing then the incoming clip with a blend strictly between 0 and 1.
AC2 (undoing_and_redoing_the_whole_edit_gives_back_the_same_project): drives edit.undo until history.get says can_undo is false (32 steps), asserts the project file text is back to the one the session opened with, redoes 32 steps and asserts the project.get text is identical to the pre-undo text. project.get returns the model in the .sub file shape with keys sorted, so that comparison is a character-for-character project-file comparison.
AC3 (frame_one_hundred_of_the_assembled_edit_renders_a_picture): renders the redone project, not the one the commands left behind. A solid red source for the outgoing half of the crossfade, green for the incoming half and white elsewhere; frame 100 composites to summary.drawn() == 2 and a centre pixel of [187, 187, 0, 255] on llvmpipe, which is the exact 50/50 red-to-green dissolve in linear light. It is non-black, and the assertions also pin that both halves contributed. With no wgpu adapter the test prints why it skipped and passes, as the other GPU tests here do.

sub-command gained dev-dependencies on sub-render, sub-time and wgpu (same feature set as sub-render) for the render half.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-command all green (3 new tests, plus the existing events, second_process, unit and doc tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Automated the phase 2 exit criterion as crates/sub-command/tests/end_to_end_edit.rs: a headless test that assembles a 20-clip, 3-track sequence with splits, trims and three crossfades entirely through JSON-RPC calls to a live Command API server, undoes and redoes all 32 commands and compares the project file text character for character, and renders frame 100 of the redone project on a headless wgpu device. Frame 100 sits halfway through a crossfade and composites to [187, 187, 0], the exact 50/50 red-to-green dissolve in linear light, so the picture is provably non-black and provably blended. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-command, all clean.
<!-- SECTION:FINAL_SUMMARY:END -->
