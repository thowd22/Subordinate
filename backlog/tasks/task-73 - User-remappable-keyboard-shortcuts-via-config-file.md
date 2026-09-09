---
id: TASK-73
title: User-remappable keyboard shortcuts via config file
status: Done
assignee:
  - '@opus-task-73'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 16:49'
labels:
  - ui
milestone: m-5
dependencies:
  - TASK-41
references:
  - docs/PLAN.md
priority: medium
ordinal: 94000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Editors migrating from Premiere or Resolve expect their bindings.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 keymap.toml in the config dir overrides defaults; invalid entries are reported not fatal
- [x] #2 A Premiere-style alternative map ships as an example
- [x] #3 Shortcut help window reflects the active map
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a keymap module to sub-ui: platform config dir resolution (XDG_CONFIG_HOME / APPDATA / Library Application Support, plus SUBORDINATE_CONFIG_DIR override), keymap.toml path.
2. Chord grammar: parse 'Ctrl+Shift+Z' style specs into egui KeyboardShortcut via Key::from_name, Ctrl/Cmd/Command mapping to Modifiers::COMMAND to match DEFAULT_BINDINGS; 'none' unbinds an action. Round-trip formatter for writing specs back.
3. Overlay loader: parse a [bindings] TOML table, rebind matching Actions on top of ShortcutMap::default_map(). Every bad entry (unknown action id, unparsable chord, wrong value type) becomes a SubError with a stable ui.keymap_* code collected into a problems list; the map still loads. A missing file is not a problem, an unreadable/malformed file yields one problem and the defaults.
4. Ship crates/sub-ui/keymaps/premiere.toml as the Premiere-style example, covered by a test that it loads with no problems and actually changes the bindings.
5. Wire it into SubordinateApp::new: load from the config dir, log every problem as a warning, keep the loaded map. Show the active map plus any keymap problems in ShortcutsWindow so the help window reflects what is in force.
6. Tests: chord round-trip, unknown action / bad chord / bad type reporting, unbinding, missing and malformed files, painted help window reflecting an overridden chord. Run fmt, clippy -D warnings, cargo test -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-ui/src/keymap.rs: the user's keymap.toml as an overlay on the shipped DEFAULT_BINDINGS, not a replacement.

Config directory: SUBORDINATE_CONFIG_DIR wins, otherwise $XDG_CONFIG_HOME/subordinate (or ~/.config) on Unix, %APPDATA%\\subordinate on Windows, ~/Library/Application Support/subordinate on macOS. config_dir_from takes the environment lookup as a parameter so the tests describe a Windows or a home-less machine without calling the unsafe set_var (unsafe_code is a workspace lint, and mutating the environment would race the other tests in the binary).

File format: a [bindings] table of stable Action::id to chord, e.g. "editing.split_at_playhead" = "Ctrl+K". Modifier words are case-insensitive; ctrl/cmd/command/super all become Modifiers::COMMAND so a file is portable between macOS and everywhere else, exactly as the shipped table writes them. Key names are egui's Key::from_name, so Left, Space, Comma and , all work. 'none' or an empty string unbinds an action (new ShortcutMap::unbind), leaving it listed in the help window with '-'. Trailing '+' is the plus key, so Ctrl+ and Ctrl++ both read as Ctrl with Plus.

Reported, never fatal: a missing file is not a problem at all; an unreadable one, malformed TOML, a [bindings] that is not a table, an unknown action id, a non-string value and an unparsable chord each become one SubError with a new stable code (ui.keymap_unreadable, ui.keymap_parse, ui.keymap_unknown_action, ui.keymap_invalid_chord) collected in LoadedKeymap::problems. Entries that parsed are still applied; rejected ones keep their shipped chord. SubordinateApp::new calls LoadedKeymap::load, logs every problem plus the existing conflict check, and holds the loaded map, so a broken keymap never stops the editor from starting.

Help window: ShortcutsWindow::show_with_problems / ui_with_problems paint the rows from the map in force and then list any problems under a 'Keymap problems' heading, so the window is both the active map and where a rejected entry is reported. show/ui stay as thin wrappers with no problems.

Example: crates/sub-ui/keymaps/premiere.toml, a Premiere-flavoured map (C for the razor, Ctrl+M for markers, Alt+Left/Right nudging, Ctrl+Alt+K for the shortcut window) with a header comment giving the install path on each platform. It is include_str!'d by the tests, which assert it parses with zero problems, has no conflicts, validates, actually differs from the defaults and still binds every action.

New dependency: toml 1 (default-features = false, features = parse, serde), already in Cargo.lock and the local registry cache, so the workspace still resolves offline.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
keymap.toml in the config directory now overrides the shipped keyboard map. crates/sub-ui/src/keymap.rs resolves the platform config directory (with a SUBORDINATE_CONFIG_DIR override), parses a [bindings] table of stable action ids to chords such as Ctrl+Shift+Z, applies each entry over ShortcutMap::default_map, and collects every rejected entry as a SubError with a new stable code (ui.keymap_parse, ui.keymap_unknown_action, ui.keymap_invalid_chord, ui.keymap_unreadable) instead of failing: a missing file changes nothing, a malformed file keeps the defaults, and a bad entry leaves the good ones applied. SubordinateApp::new loads it at startup and logs the problems; the shortcut help window paints the map in force and lists the problems under it. crates/sub-ui/keymaps/premiere.toml ships as the Premiere-style example, with install paths in its header.

Verified with cargo fmt --all --check (clean), cargo clippy --workspace --all-targets -- -D warnings (clean) and cargo test -p sub-ui (132 tests, 22 new: chord round-trips over every shipped chord, case-insensitive and platform-neutral modifiers, the plus key, bad chords, unknown actions, wrong value types, unbinding with 'none', malformed TOML, a non-table [bindings], missing and unreadable files, the environment-driven config path, and two headless egui paints proving the help window draws the remapped chords rather than the defaults and shows a rejected entry's code). The example map is include_str!'d and asserted to load with no problems, no conflicts, and every action still bound.
<!-- SECTION:FINAL_SUMMARY:END -->
