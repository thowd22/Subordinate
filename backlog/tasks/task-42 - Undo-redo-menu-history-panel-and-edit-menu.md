---
id: TASK-42
title: 'Undo/redo menu, history panel and edit menu'
status: Done
assignee:
  - '@opus-task-42'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 16:45'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-41
  - TASK-4.1
references:
  - docs/PLAN.md
priority: medium
ordinal: 63000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users need visible history; the engine already supports it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Edit menu with Undo/Redo showing the command name
- [x] #2 History panel lists commands and allows jumping to a point
- [x] #3 Redo stack clears on new command as expected
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-ui/src/history_panel.rs: HistoryList (labels + current position built from sub_edit::History), HistoryAction (Undo/Redo by n steps, with to_position math and apply against a real History), edit_menu_ui rendering Edit > Undo <name> / Redo <name> with chord labels from ShortcutMap, and HistoryPanel window listing every entry with click-to-jump.
2. Export the new types from sub-ui lib.rs; keep the panel free of editing logic (it only asks for undo/redo, which the Command API performs).
3. Tests: unit tests for label and position math; headless egui integration test (tests/history_panel.rs) driving the menu and panel over a real History, covering jump-to-a-point in both directions and the redo stack clearing when a new command is applied.
4. Verify with cargo fmt --check, clippy -D warnings and cargo test -p sub-ui -p sub-edit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-ui/src/history_panel.rs.

- HistoryList reads a sub_edit::History into the labels of every step (undo stack oldest first, then the redo stack in redo order) plus the current position, so one type serves both the menu and the panel. undo_menu_label()/redo_menu_label() produce 'Undo <command name>' and fall back to the bare verb when the stack is empty.
- edit_menu_ui draws the Edit menu with those two entries, each disabled when its stack is empty and each showing its chord from ShortcutMap (Ctrl+Z / Shift+Ctrl+Z), and returns a single-step HistoryAction.
- HistoryPanel lists the open state and one row per step, dims the undone ones, marks the current position, records each row's rect, and turns a click into HistoryAction::to_position.
- HistoryAction is Undo/Redo by n steps rather than a target index: a jump is the same undo/redo calls a run of Ctrl+Z would make, so nothing here bypasses the Command API and a jump stays undoable. perform() stops early rather than failing if the list was drawn one command out of date.

Verification: cargo test -p sub-ui (99 unit + 8 new integration + 5 doctests, all pass), cargo fmt --all --check clean, cargo clippy --workspace --all-targets -- -D warnings clean. AC1 is proven by the menu labels test and the headless menu paint; AC2 by tests/history_panel.rs jumping_back_and_forward_lands_on_the_clicked_step and clicking_a_row_in_a_painted_panel_asks_for_that_jump, which click a painted row and then apply the jump against a real History holding real track commands; AC3 by a_new_command_clears_the_redo_side_of_the_list, which undoes twice, applies a new command and shows the redo rows gone from the list.

Scope note: the panel and menu are not yet mounted in app.rs, following the same convention as the timeline panel, track headers and sequence tabs — the app shell owns no Project or Engine yet, so there is no history for it to show. Mounting them is one call each (edit_menu_ui in the top bar, HistoryPanel::show) once the shell owns an engine.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the Edit menu and history panel to sub-ui as a new history_panel module: HistoryList turns a sub_edit::History into labelled steps plus a position, edit_menu_ui draws Undo/Redo named after the command they act on with their chords, and HistoryPanel lists every step (undone ones dimmed) with click-to-jump. A jump is expressed as a run of undo or redo steps performed through the existing history, so no widget bypasses the Command API. Verified with cargo test -p sub-ui (8 new headless egui integration tests over a real History plus unit and doc tests), cargo fmt --all --check and cargo clippy --workspace --all-targets -- -D warnings, all clean.
<!-- SECTION:FINAL_SUMMARY:END -->
