---
id: TASK-32
title: Split at playhead and razor tool
status: Done
assignee:
  - '@opus-task-32'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 16:36'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-30
references:
  - docs/PLAN.md
priority: high
ordinal: 53000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Cuts are the basic verb of editing.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Ctrl+K splits selected clips (or all clips under the playhead when nothing is selected)
- [x] #2 Razor tool splits the clicked clip at the cursor time
- [x] #3 Split respects snapping and is undoable
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers this: an interaction test that splits at the playhead and asserts the two resulting clips, plus a snapshot of the razor tool state
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a sub-ui split module: plan a cut from an instant plus the selection (selection decides, else every editable track under the playhead), mint tail ClipIds up front, emit SplitClip commands and apply_split inside one History group.
2. Add a Tool enum (Select/Razor) to TimelinePanel; a razor press cuts the clip under the pointer at the snapped instant, painting a cut line before the press.
3. Raise both through TimelineResponse (clip_split, split_refused) and wire Ctrl+K (Action::SplitAtPlayhead) in app.rs to request_split_at_playhead.
4. Tests: unit tests in split.rs for the planning rules, and an egui_kittest file tests/timeline_split.rs on the shared harness covering the playhead cut (asserting the two resulting clips and one undo), razor click, marker snapping and locked tracks, plus a razor-state snapshot.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

- New `crates/sub-ui/src/split.rs`: `plan_split` (selection decides what a Ctrl+K cut touches; with nothing selected, every clip of every editable track the playhead falls strictly inside), `plan_split_clip` for the razor, `SplitCut`/`SplitGroup` value types, `SplitRefusal` with stable ids and `ui.clip_split_refused`, and `apply_split`, which puts the whole cut in one `History` group so a through-edit is one undo. Tail ClipIds are minted when the cut is planned and handed to `SplitClip::tail_id`, so preview, command and test all name the same clip. All arithmetic is RationalTime; nothing here mutates a project.
- `TimelinePanel`: a `Tool` enum (Select/Razor). A razor press cuts the clip under the pointer at the snapped instant (candidates include the playhead), painting a yellow cut line with a blade before the press; the press becomes a `Gesture::Cut` so holding the button cuts once. `request_split_at_playhead` queues the Ctrl+K cut for the next painted frame, which is when the panel has the sequence to plan against. Both reach the caller as `TimelineResponse::clip_split` / `split_refused`.
- `app.rs` wires Action::SplitAtPlayhead, and two new actions SelectTool (V) and RazorTool (C) so the razor is reachable at all; the planned cut is logged rather than applied for the same reason a clip drag is (the app owns no engine handle yet, TASK-12 wiring).
- `keymaps/premiere.toml`: split_at_playhead moves back to Ctrl+K (Premiere's Add Edit) and C now selects the razor tool, which is what C actually is in Premiere; the two keymap tests that named the old mapping were updated.

Cut semantics: a clip is only cut where the instant falls strictly inside it. A cut that lands on a clip's own head is skipped rather than refused (there is already an edit there), so a through-edit that lines up with one clip's head still cuts the others. Snapping applies to the razor exactly as it does to a scrub, so a cut can land on a marker, a clip edge or the playhead.

Verification (WSL, no sudo; GStreamer env sourced from ~/.cache/subordinate/env-gst.sh; a software wgpu adapter is available here so the snapshot really rendered)
- cargo fmt --all --check: clean
- cargo clippy --workspace --all-targets -- -D warnings: clean
- cargo test -p sub-ui: 21 test binaries, all green, including the 9 new unit tests in split.rs and the 8 in tests/timeline_split.rs
- New committed snapshot crates/sub-ui/tests/snapshots/timeline_razor_tool.png (the marker in the scene has a fixed MarkerId because its swatch is derived from the id, as tests/markers.rs does)
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added splitting to the timeline: a new sub-ui split module plans a cut from an instant plus the selection and applies it as one undoable History group of SplitClip commands, and the timeline panel gained a Tool enum whose razor cuts the clicked clip at the snapped instant while Ctrl+K cuts at the playhead (the selection decides what it touches, else every editable track under it). Both reach the caller through TimelineResponse, with V and C selecting the two tools. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-ui (all green), covering 9 new unit tests, 8 egui_kittest tests in tests/timeline_split.rs and a new committed razor-tool snapshot.
<!-- SECTION:FINAL_SUMMARY:END -->
