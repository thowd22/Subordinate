---
id: TASK-33
title: 'Sequence tabs: create, rename, delete, switch'
status: Done
assignee:
  - '@opus-task-33'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 16:04'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-28
  - TASK-4.3
references:
  - docs/PLAN.md
priority: high
ordinal: 54000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Multiple timelines per project is an explicit MVP requirement (§2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Sequence tabs above the timeline; new sequence dialog with settings
- [x] #2 Switching sequences preserves per-sequence zoom, scroll and playhead
- [x] #3 Deleting the last sequence is refused
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-ui/src/sequence_tabs.rs: SequenceTabs (tab strip above the timeline), SequenceTabAction (Switch/Create/Rename/Delete intents the host turns into sub-edit Commands, keeping the UI free of mutation), NewSequenceDialog with name/resolution/frame rate/sample rate producing a validated SequenceSettings.
2. Per-sequence view state: SequenceViewState captures zoom, horizontal scroll_px, lane scroll and playhead from TimelinePanel + ViewerState on leaving a tab and restores them (at the new sequence's timebase) on returning; unseen sequences start at their own defaults.
3. Refuse deleting the last sequence in the tab strip with a new stable code ui.last_sequence (the DeleteSequence command must stay permissive because it is the inverse of CreateSequence); unknown ids report ui.unknown_sequence.
4. Export the module from lib.rs, add codes, add headless tests (egui Context::run_ui) for tab painting, switch round-trip of zoom/scroll/playhead, dialog validation and last-sequence refusal.
5. cargo fmt --check, clippy -D warnings, cargo test -p sub-ui; finalize task.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-ui/src/sequence_tabs.rs: SequenceTabs (the tab strip drawn above the timeline panel), SequenceTabAction (Switch/Create/Rename/Delete), NewSequenceDialog (name, canvas, frame rate, sample rate, validated through SequenceSettings::new) and SequenceViewState (per-sequence zoom, horizontal scroll, lane scroll and playhead).

Decisions:
- The strip mutates nothing. It reports an intent that the host turns into CreateSequence / RenameSequence / DeleteSequence, so every project change stays an undoable Command; only Switch is view-only state.
- The last-sequence refusal lives in the strip (new code ui.last_sequence), not in the DeleteSequence command: that command is also the inverse of creating the very first sequence, so making it refuse a one-sequence project would break undo of CreateSequence. Unknown ids report ui.unknown_sequence.
- Switching captures the outgoing tab's view state and restores the incoming one at that sequence's own timebase (ViewerState::set_rate rescales the playhead; the duration is re-read so a shortened sequence still lands on a real frame). Zoom stays an exact pixels-per-frame fraction and the playhead a RationalTime; the only float kept is the lane scroll, which is a screen measurement.
- TimelinePanel gained restore_lane_scroll, an unclamped setter, because a restore happens before egui has told the panel how tall the lane viewport is this frame.

Not done (out of scope, not a criterion): the strip is not yet mounted in SubordinateApp. The app shell still owns a single Sequence rather than a Project and does not mount the timeline panel either (same state TASK-28 left it in); wiring panels to the engine's command queue belongs to the app-shell work, and sub-ui deliberately has no sub-edit dependency.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui green (69 unit + 4 new headless sequence_tabs integration tests + 2 timeline paint + 2 doc tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Sequence tabs land in sub-ui as a self-contained strip: SequenceTabs draws one tab per sequence above the timeline, opens a new-sequence dialog with canvas, frame-rate and sample-rate settings, renames a tab in place and deletes one from its context menu, emitting SequenceTabAction values the host turns into the existing sub-edit commands so every mutation stays undoable. Switching tabs saves and restores that sequence's zoom, horizontal scroll, lane scroll and playhead at its own timebase, and deleting the only remaining sequence is refused with the new ui.last_sequence code (the DeleteSequence command stays permissive because it is the inverse of creating the first sequence). Verified headlessly with four new egui Context::run_ui integration tests plus module unit tests: cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-ui all pass.
<!-- SECTION:FINAL_SUMMARY:END -->
