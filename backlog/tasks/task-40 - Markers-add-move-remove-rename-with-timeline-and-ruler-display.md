---
id: TASK-40
title: 'Markers: add, move, remove, rename with timeline and ruler display'
status: Done
assignee:
  - '@opus-task-40'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 14:07'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-29
  - TASK-4.4
references:
  - docs/PLAN.md
priority: medium
ordinal: 61000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Markers are the simplest way for agents and analyzers to annotate a timeline.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 M adds a marker at the playhead; markers render on the ruler with names and colours
- [x] #2 Drag moves, double-click renames, Delete removes; all undoable
- [x] #3 Markers are snap targets
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers markers: a committed snapshot of the ruler and timeline with markers, and an interaction test that adds, moves or renames one and asserts the command
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-edit: add RenameMarker (kind marker.rename) alongside AddMarker/MoveMarker/RemoveMarker, register it in the builtin registry and cover it with tests; nothing else in the marker command set is missing.
2. sub-ui: new module markers.rs holding MarkerAction (Add/Move/Rename/Remove) with into_command(sequence) so every marker gesture is one undoable Command, MarkerState (selection, drag, inline rename) and a deterministic per-marker colour drawn from a fixed palette keyed by MarkerId (the model carries no colour field and decision-3 keeps colour tags out of the entity model, so the palette lives in the UI).
3. timeline_panel: paint marker flags with names and colours on the ruler plus a tinted guide line down the lanes; hit-test the ruler so a press on a flag drags rather than scrubs; double-click opens the inline rename editor; Delete removes the selected marker; M at the playhead adds one. Marker actions come back on TimelineResponse alongside the track actions.
4. Confirm markers are already snap targets (snapping.rs collects them) and keep a test that proves it through the panel.
5. Tests: unit tests in markers.rs, an egui_kittest interaction test in crates/sub-ui/tests/markers.rs on the shared harness that adds, drags and renames a marker and asserts the resulting command, and a committed ruler+timeline snapshot with markers.
6. Verify with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test for sub-edit and sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Marker gestures on the ruler, and the one command each of them is.

sub-edit: added RenameMarker (kind marker.rename) beside AddMarker/MoveMarker/RemoveMarker; it swaps the name and returns itself carrying the old one, so undo lands on the same marker with its id, span and note untouched. Registered in builtin_registry and regenerated docs/schema/command-api.json (SUB_UPDATE_SCHEMA=1), which is what puts it on the Command API and in the MCP tool list. Covered in crates/sub-edit/tests/param_marker_media_bin_commands.rs by the existing round_trip helper, so its inverse is proved against the bytes of the project file rather than a field read.

sub-ui: new crates/sub-ui/src/markers.rs holds MarkerAction (Add/Move/Rename/Remove) with into_command(sequence) and MarkerState (selection, drag, inline rename). The panel owns no editing logic: every gesture comes back on TimelineResponse::marker_actions for the caller to apply, exactly as TrackAction already does.

Colour: Marker carries no colour field and decision-3 keeps the model's colour to the colour-space tags on media and sequences, so adding a swatch to the entity would be a project-file change rather than a UI one. markers::marker_color instead picks deterministically from a six-swatch palette using the low byte of the marker's UUIDv7 (the leading bits are a millisecond timestamp, so markers dropped in one session would otherwise share a swatch). The colour therefore survives save, load and undo, and is the same on every machine.

Gestures: a press on a flag claims the press (handle_markers runs before handle_scrub) and drags the marker, snapped like everything else dropped on the timeline but with the marker's own ends removed from the candidates; the grab offset inside the flag is kept in points so the marker does not jump to the pointer. A span marker keeps its exact duration when it moves (TimeRange::new(start, old.duration()), never a float). A double-click opens an inline TextEdit seeded with the old name and selected whole; Enter or losing focus commits, Escape abandons, and an empty or unchanged name raises nothing. Delete or Backspace removes the selection unless a name is being typed into. M goes through TimelinePanel::add_marker_at_playhead, which mints the marker and raises the add on the next frame; app.rs wires Action::AddMarker to it.

Painting: a coloured flag hanging from the bottom of the ruler (clear of the playhead's head at the top), a bar across what a span marker covers, the name on a dark plate so it stays readable under a timecode label, a white outline on the selected flag, and a faint guide line of the same colour down the lanes.

AC #3 needed no new code: snapping::collect_candidates already offers both ends of every marker, and a_marker_is_a_snap_target_for_the_playhead proves it through the panel while a_dragged_marker_snaps_onto_a_cut proves the other direction.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test --workspace passes. The nine tests in crates/sub-ui/tests/markers.rs drive the pointer through the shared kittest harness and apply what the panel raised through a real History, so each assertion is on the project after the command and after undo. That harness is built with step_dt 1/60 rather than the shared default of 1/4 second, because two clicks four frames a second apart are half a second apart and would never be a double-click. Snapshots: new timeline_markers.png; harness_timeline_panel.png and timeline_playhead.png were re-recorded because the committed sample project already carries an 'act two' marker that the ruler now draws.

Not done, and outside the acceptance criteria: clip markers (MarkerTarget::Clip) are edited by the same commands but the ruler only edits the sequence's; marker notes have no editor yet; and the panel's actions still reach app.rs as log lines because the app has no engine handle yet, exactly as the track actions do.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Markers are now editable from the timeline ruler and every gesture is one undoable command. Added RenameMarker (marker.rename) to sub-edit and regenerated docs/schema/command-api.json; added crates/sub-ui/src/markers.rs with MarkerAction/MarkerState and a deterministic per-marker colour, and taught the timeline panel to paint marker flags, names, span bars and guide lines and to drag, rename in place, select and delete them, with M dropping one at the playhead. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test --workspace, including nine new kittest tests in crates/sub-ui/tests/markers.rs that drive the pointer and assert on the project after the command and after undo, plus a committed timeline_markers.png snapshot.
<!-- SECTION:FINAL_SUMMARY:END -->
