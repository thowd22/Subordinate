---
id: TASK-34
title: 'Track header controls: add, remove, reorder, rename, mute, lock'
status: Done
assignee:
  - '@opus-task-34'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 15:59'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-28
  - TASK-4.3
references:
  - docs/PLAN.md
priority: medium
ordinal: 55000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Track management UI over the existing track commands.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Track header shows name, mute and lock toggles; right-click menu adds, removes, renames and reorders
- [x] #2 Locked tracks render clips dimmed and refuse edits
- [x] #3 All actions are undoable
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub-edit dependency to sub-ui so header controls can name the commands they raise.
2. New module crates/sub-ui/src/track_header.rs: TrackAction (add, remove, rename, reorder, mute, lock) with into_command mapping onto the existing sub-edit track commands, and TrackHeaderState holding the inline rename buffer plus the egui widgets: name label, M and L toggles, and a right-click menu that adds, renames, removes and moves tracks up/down.
3. Wire the headers into TimelinePanel::ui: interactive child UIs over the header rects, collecting actions into a new TimelineResponse { response, actions }; keep the lanes themselves painter-only.
4. Locked tracks: dim the clip rectangles and the lane, and gate clip interaction behind a clip_edits_allowed helper; command layer already refuses with edit.track_locked.
5. Tests: headless egui tests for the header widgets and actions, unit tests for the action-to-command mapping applied through History (apply + undo restores), a test that a clip command on a locked track fails with edit.track_locked, and a dimming test.
6. Verify with cargo fmt --check, clippy pedantic -D warnings and cargo test -p sub-ui -p sub-edit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in sub-ui.

New crates/sub-ui/src/track_header.rs: TrackAction (Add, Remove{force}, Rename, Reorder, SetMuted, SetLocked) with into_command() mapping each variant onto the existing sub-edit track command, so the header holds no editing logic and every action lands on the undo stack. TrackHeaderState owns only the inline rename buffer; the rest is read from the Track each frame. HeaderLayout computes the hit rectangles (name, kind, mute, lock) rather than letting egui lay them out, which is what makes the controls testable headlessly. menu_entries()/empty_menu_entries() build the right-click menu: add video/audio track, rename, move up/down (only where there is somewhere to move), remove (worded and forced when the track still holds clips).

TimelinePanel: added a header widget pass over the painted header plates (the lanes stay painter-only; the per-widget cost note in docs/PLAN.md 5.7 is about clips, not the tens of tracks), ui() now returns TimelineResponse { response, actions }, and layout()/header_rect() expose where the panel last painted. sub-ui gained a sub-edit dependency for the action-to-command mapping.

Locked tracks: lane background and clip fill, outline, trim bars and name are all painted through dim() at 0.45 when clip_edits_allowed(track) is false, and the panel offers no clip edit on such a track. The command layer refuses clip edits routed through sub-edit's track_for_clip_edit with edit.track_locked.

Finding, not fixed here (belongs to TASK-4.2, no follow-up task created): the primitive clip commands in sub-edit/src/clip.rs (AddClip, MoveClip, TrimClipIn/Out, SplitClip, RippleDelete) look tracks up through their own track_of/track_mut helpers rather than commands::track_for_clip_edit, so they currently succeed on a locked track. Commands that do go through the lock lookup (SetClipParams, markers) are refused as documented. Worth raising with the user before changing, because RestoreTrackItems is an inverse and must stay able to run.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui -p sub-edit all green (sub-ui 72 unit + 8 track_headers + 2 timeline_panel_paint + 3 doc tests). The headless egui tests in crates/sub-ui/tests/track_headers.rs synthesise real pointer and keyboard input: clicking the M and L toggles raises SetMuted/SetLocked and each applies and undoes through History; right-clicking a header opens the menu with the expected entries; clicking 'Move up' raises a Reorder that applies and undoes; renaming from the menu types a new name over the selected old one and commits it on Enter; a locked track paints its clips dimmed and SetClipParams on it fails with edit.track_locked.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Track header controls over the existing sub-edit track commands. crates/sub-ui/src/track_header.rs adds TrackAction and TrackHeaderState: a header shows the track name (double-click or menu to rename inline), its kind, and mute and lock toggles, with a right-click menu that adds video or audio tracks, renames, moves the track up or down and removes it (forced, and worded as such, when it still holds clips). Every control raises a TrackAction which into_command() turns into AddTrack, RemoveTrack, RenameTrack, ReorderTrack, SetTrackMuted or SetTrackLocked, so nothing bypasses the Command API and everything undoes. TimelinePanel now runs those widgets over its header plates and returns TimelineResponse { response, actions }; locked tracks paint their lane and clips dimmed and are gated by clip_edits_allowed, and the command layer refuses clip edits on them with edit.track_locked. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test -p sub-ui -p sub-edit, including headless egui tests that click the toggles, open the menu, choose Move up and type a rename, each applied and undone through History.
<!-- SECTION:FINAL_SUMMARY:END -->
