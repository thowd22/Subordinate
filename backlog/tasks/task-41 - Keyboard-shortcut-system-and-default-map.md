---
id: TASK-41
title: Keyboard shortcut system and default map
status: Done
assignee:
  - '@opus-task-41'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 15:55'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-28
references:
  - docs/PLAN.md
priority: medium
ordinal: 62000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Editors live on the keyboard. A central map keeps shortcuts consistent and user-remappable (phase 5).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Central action registry with default bindings for JKL, I/O, comma/period, Ctrl+K, Ctrl+Z/Shift+Ctrl+Z, S, M, space
- [x] #2 Conflicts are detected at startup and logged
- [x] #3 A shortcuts help window lists every binding
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-ui/src/shortcuts.rs: an egui-free-in-spirit action registry — Action enum (transport, marking, editing, view) with stable ids, labels and categories; Binding = action + egui::KeyboardShortcut; ShortcutMap with default_map() covering JKL, I/O, comma/period, Ctrl+K, Ctrl+Z / Shift+Ctrl+Z, S, M, Space plus the arrow/Home/End viewer keys already in use.
2. Conflict detection: ShortcutMap::conflicts() groups chords bound to more than one action; validate() returns SubError ui.shortcut_conflict with details; log_conflicts() warns via log at startup. Wire the check into SubordinateApp::new so it runs at startup.
3. Input: ShortcutMap::poll(ctx) matches key events with exact modifiers (most specific first), consumes matched events and returns the actions fired, skipping frames where egui wants text input.
4. Help window: ShortcutsWindow with rows(&map) grouped by category listing every binding, plus a show(ctx, &map) egui window; bind it to F1 and add a toolbar button in app.rs.
5. Re-point viewer::action_for_key at the central map so there is a single source of truth for the playhead keys.
6. Tests: unit tests for defaults, conflict detection/validate error code, poll matching exactness and repeats; headless egui paint test asserting the help window emits a row for every binding.
7. Verify with cargo fmt --check, clippy --workspace --all-targets -D warnings, cargo test -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-ui/src/shortcuts.rs, the central action registry: Action (18 variants) with a stable id such as transport.play_forward or editing.undo for the phase-5 remapping file and plugin shortcut registration, plus a label and a category; DEFAULT_BINDINGS is a const table pairing each action with an egui KeyboardShortcut. Defaults cover J/K/L, Space, I/O, comma/period, Ctrl+K, Ctrl+Z, Shift+Ctrl+Z, S and M, plus the arrow and Home/End playhead keys the viewer already used. Ctrl is written as Modifiers::COMMAND so it is Cmd on macOS without a second table.

Matching is exact on modifiers (Modifiers::matches_exact) rather than egui consume_shortcut, which ignores extra Shift/Alt: Shift+Ctrl+Z never falls through to undo and Shift+S does not toggle snapping. ShortcutMap::poll consumes the matched key events from egui's queue, so a bound chord is handled once and no panel sees it twice, and it returns nothing while a text field holds the keyboard. Key repeats fire again, which is what frame stepping wants.

Conflicts: ShortcutMap::conflicts groups chords claimed by more than one action; validate() returns a SubError with the new stable code ui.shortcut_conflict and one detail per chord naming the action ids; log_conflicts() warns through log and returns the count. SubordinateApp::new calls log_conflicts once at startup, because a conflict is a configuration problem rather than a reason to refuse to start. The help window (ShortcutsWindow, bound to F1 and a toolbar button) is built from help_rows(), which projects the whole map into category-grouped rows, so it is exhaustive by construction. Nothing here mutates the project, so nothing here is a Command; the editing actions become undoable Commands where they are applied, and actions with no panel yet are logged at debug and dropped. viewer::action_for_key now reads the same table through shortcuts::default_action_for.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui green (79 unit, 2 integration, 3 doc tests). Evidence per criterion: AC1 the_documented_editor_keys_are_bound and the_default_map_binds_every_action_exactly_once; AC2 a_double_bound_chord_is_a_conflict_with_a_stable_code and every_conflict_is_logged_as_a_warning (a capturing log::Log sink asserts the warning text); AC3 the_painted_help_window_lists_every_binding, a headless egui paint asserting every action label, every chord label and every category heading is actually drawn. Caveat on AC2: detection and logging are proven by tests, but the one-line call from SubordinateApp::new is code presence only, since constructing the app needs an eframe CreationContext and a GPU adapter that this environment lacks.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added a central keyboard map in crates/sub-ui/src/shortcuts.rs: an Action registry with stable ids, a const default binding table (JKL, Space, I/O, comma/period, Ctrl+K, Ctrl+Z, Shift+Ctrl+Z, S, M and the playhead keys), exact-modifier lookup, event-consuming polling, conflict detection that logs at startup and returns a SubError with the new ui.shortcut_conflict code, and a help window built from the map so it lists every binding. app.rs runs the map before any panel reads the keyboard and opens the help window from F1 or a toolbar button; viewer::action_for_key now reads the same table. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-ui (84 tests, including a capturing-logger conflict test and a headless egui paint asserting the help window draws every binding).
<!-- SECTION:FINAL_SUMMARY:END -->
