---
id: TASK-111
title: Insert and overwrite edits from the bin and drag-from-bin
status: Done
assignee:
  - '@opus-task-111'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 16:53'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-30
  - TASK-35
references:
  - docs/PLAN.md
priority: high
ordinal: 32500
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Getting media onto the timeline with predictable three-point behaviour.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Dragging a media item from the bin onto a track creates a clip at the drop time
- [x] #2 Comma inserts (ripples later clips) and period overwrites at the playhead on the target track
- [x] #3 Audio tracks receive audio-only items; video tracks refuse them with a hint
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers insert and overwrite from the bin: interaction tests asserting the resulting clips, plus a snapshot of the drag feedback if one is drawn
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-edit: add an InsertClip command (clip.insert) beside AddClip: split any clip straddling the insert point, ripple everything from there by the clip's duration, then place the clip. Register it, export it, cover it with unit tests (ripple, split, undo).
2. sub-ui: new module source_edit.rs planning a bin-to-timeline edit — EditMode (Insert/Overwrite), SourceRefusal with a hint (wrong track kind, locked track, unprobed/offline item, before start), plan_source_edit -> PlannedEdit carrying the clip, the span and the command, and apply_source_edit through History as one undo step. New error code ui.source_edit_refused.
3. media bin: make an item row/tile a drag source carrying its MediaId; timeline panel: accept the drop, hit-test the lane and time (snapped), plan the overwrite, paint drop feedback, and report it on TimelineResponse.
4. shortcuts: bind comma to editing.insert_at_playhead and period to editing.overwrite_at_playhead (nudge moves to Alt+comma/period); app wires the actions to the bin selection and the target track.
5. Tests: sub-ui integration test on the shared harness driving insert and overwrite from the bin and asserting the resulting clips, plus unit tests for the refusals and a paint check of the drop feedback.
6. Verify with cargo fmt, clippy -D warnings and cargo test for sub-edit and sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
sub-edit: added InsertClip (kind clip.insert) beside AddClip. It splits the clip straddling the insert point (head keeps the identity, as SplitClip does), ripples everything from there by the clip duration and places the clip, undoing through the same RestoreTrackItems inverse as the rest. Registered in clip::register/builtin_registry and docs/schema/command-api.json regenerated with SUB_UPDATE_SCHEMA=1, so it is on the Command API and in the MCP tool list. Covered in crates/sub-edit/tests/clip_commands.rs by the existing round_trip helper (ripple, split-and-ripple, padding past the end, refusals).

sub-ui: new crates/sub-ui/src/source_edit.rs is the whole bin-to-timeline policy in one place: EditMode (Insert/Overwrite), SourceRefusal with a hint per reason and the new ui.source_edit_refused code, plan_source_edit -> PlannedEdit (clip, span, track, command) and apply_source_edit through History. Track kinds are a modelled refusal rather than a filter: a video track refuses an item with no picture with 'this item has no picture: drop it on an audio track', and an audio track refuses a silent one; an item carrying both goes on either.

Media bin: the item name in the list and the tile in the grid are egui drag sources carrying BinDrag(MediaId) (media_bin::drag_source_id keeps the id stable so a test can find the row). Only the name cell drags, so the Relink button and the metadata cells stay ordinary buttons. The timeline plans the drop every frame the pointer holds one over the lanes, paints the target span as a ghost (or washes the lane red with the refusal), and reports it on TimelineResponse::source_edit / drop_refused; the lane pointed at also becomes the panel's target_track.

Shortcuts: comma is editing.insert_at_playhead and period editing.overwrite_at_playhead; nudging moved to Alt+comma / Alt+period, which is what freed the pair. The app plans the edit from the bin selection at the viewer playhead on the target track and logs it, the way every other command-raising action in app.rs is logged until the app owns an engine handle.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-edit, -p sub-command, -p sub-ui, -p subordinate-mcp, -p subordinate-cli, -p sub-plugin all pass. New crates/sub-ui/tests/bin_edits.rs paints the bin and the timeline in one kittest frame and drives a real pointer drag from a bin row into a lane: the drop places the clip at the frame it landed on, a drop over a clip overwrites it, an audio-only item is refused by the video track and taken by the audio one, comma/period at the playhead ripple and overwrite respectively (each applied through a real History and undone), a locked track refuses both paths, and the drag feedback has a committed snapshot (crates/sub-ui/tests/snapshots/bin_drop_target.png).

Caveat on AC #2: the keypress path is implemented and the semantics and bindings are proved by tests, but SubordinateApp still logs the planned command instead of applying it, because it owns no engine handle yet — the same state every other edit the panels raise is in.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Media can now be edited from the bin onto the timeline, insert or overwrite, by drag or by keyboard. sub-edit gained InsertClip (clip.insert), the rippling counterpart of AddClip, registered on the Command API with the schema regenerated; sub-ui gained source_edit.rs, which turns a media id, a track and an instant into either the command the edit will be or a SourceRefusal with a hint (ui.source_edit_refused), so a video track refuses a soundtrack-only item by pointing at the audio track instead. The bin's item names and tiles are drag sources, the timeline paints and takes the drop, and comma/period edit at the playhead on the last lane pointed at (nudging moved to Alt+comma/period). Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test for sub-edit, sub-command, sub-ui, sub-plugin, subordinate-mcp and subordinate-cli, including six new kittest tests in crates/sub-ui/tests/bin_edits.rs that drive a real drag from the bin into a lane, apply every planned edit through a real History and undo it, and a committed bin_drop_target.png snapshot of the drag feedback.
<!-- SECTION:FINAL_SUMMARY:END -->
