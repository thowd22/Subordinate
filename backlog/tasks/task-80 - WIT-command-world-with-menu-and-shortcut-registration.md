---
id: TASK-80
title: WIT command world with menu and shortcut registration
status: Done
assignee:
  - '@opus-task-80'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 01:08'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
  - TASK-42
references:
  - docs/PLAN.md
priority: high
ordinal: 101000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Composite editing operations built from primitives (§6.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 command world exports commands() -> list<CommandDesc { id, title, shortcut? }> and run(id, context) that can call the host command-api
- [x] #2 Host registers commands in a Plugins menu and the shortcut registry
- [x] #3 A plugin command run is one undo group
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. WIT: add a `command-menu` interface (command-desc {id,title,shortcut?}, command-context {project,args}) and a richer world `commands` exporting commands() -> list<command-desc> and run(id, context), importing command-api. The minimal `command` world of TASK-9 stays as it is so the spike and the SDK keep compiling.
2. sub-plugin: generate host bindings for the new world (reusing the existing types/command-api modules via bindgen `with`), and add a `menu` module: PluginCommand, PluginCommandRegistry (register/replace per plugin, validate ids and titles, reject duplicates, expose menu entries in stable order) with new stable plugin.* codes.
3. sub-plugin: run_in_undo_group(engine, command, run) wrapping a plugin command run in EngineHandle::begin_group/commit_group, aborting the group when the run fails, so one plugin run is exactly one undo step (adds a sub-edit dependency).
4. sub-ui: a `plugins` module registering the registry's commands into the Plugins menu and the shortcut registry: chords parsed with keymap::parse_chord, rejected with stable ui.* codes when invalid or already claimed by a host action or another plugin command; plugins_menu_ui mirrors edit_menu_ui; poll/take_commands consume key events after the host map.
5. Tests: WIT world links with every import satisfied; registry validation and ordering; a real sub-edit Engine proving two run-command mutations inside one plugin run collapse to one undo step; headless egui frames for the Plugins menu and the shortcut registration/conflict paths.
6. cargo fmt --check, clippy --workspace --all-targets -D warnings, cargo test -p sub-plugin -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented as a second world rather than a change to the existing one: wit/subordinate-plugin.wit keeps the minimal `command` world of TASK-9 (the SDK and the TASK-9 spike guests still compile against it) and gains a `command-menu` interface (command-desc {id, title, shortcut?}, command-context {project, args}) plus a `commands` world exporting commands() and run(id, context) over the same command-api import. sub-plugin generates the second world with bindgen `with:` pointing at the existing types/command-api modules, so both worlds share one set of Rust types and one Host impl.

sub-plugin::menu holds the host half: PluginCommandRegistry validates ids and titles, qualifies each command as <plugin>/<command>, replaces a plugin's entries wholesale on reload, and rejects a bad batch whole with new stable codes (plugin.invalid_plugin_id, plugin.invalid_command_id, plugin.invalid_command_title, plugin.invalid_shortcut, plugin.duplicate_command, plugin.unknown_command). run_as_undo_group wraps a run in EngineHandle::begin_group/commit_group and aborts the group when the run fails, keeping the plugin's own error.

sub-ui::plugins is the editor half: PluginMenu::register turns the registry into menu entries and keyboard bindings, parsing requested chords with keymap::parse_chord. A chord the editor's own map already claims, or an earlier plugin holds, is refused with ui.plugin_shortcut_conflict (ui.plugin_invalid_chord when it is not a chord at all) and the command keeps its menu entry without a shortcut; plugins_menu_ui mirrors edit_menu_ui and returns the qualified id chosen. sub-ui now depends on sub-plugin.

Verification (Linux, no GPU): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-plugin -p sub-ui all green, including the commands-world linker test, the registry tests, the plugins menu tests painted headlessly with egui run_ui, and one_plugin_command_run_is_one_undo_step / a_failed_run_rolls_its_group_back_and_keeps_its_own_error against a real sub-edit Engine.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the `commands` WIT world with menu and shortcut registration (commands() -> list<command-desc{id,title,shortcut?}> and run(id, context) over the existing command-api import), the host-side PluginCommandRegistry and undo-group runner in sub-plugin, and the Plugins menu plus plugin shortcut registration in sub-ui. Verified with cargo fmt --check, clippy --workspace --all-targets -D warnings, and cargo test -p sub-plugin -p sub-ui: the new world links with every import satisfied, the registry answers the menu and keyboard map (with host chords winning conflicts), and a plugin run applying two primitives is a single labelled undo step that rolls back whole when it fails.
<!-- SECTION:FINAL_SUMMARY:END -->
