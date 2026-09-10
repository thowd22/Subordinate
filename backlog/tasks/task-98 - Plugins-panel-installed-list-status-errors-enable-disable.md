---
id: TASK-98
title: 'Plugins panel: installed list, status, errors, enable/disable'
status: Done
assignee:
  - '@opus-task-98'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 10:14'
labels:
  - ui
  - plugins
milestone: m-6
dependencies:
  - TASK-85
  - TASK-43
references:
  - docs/PLAN.md
priority: medium
ordinal: 119000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Non-agent users need to manage plugins too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Panel lists plugins with version, worlds, capabilities and load status
- [x] #2 Enable, disable, remove and open-folder actions
- [x] #3 Reload errors display inline
- [x] #4 Hot-reload errors from --dev installs display inline in the panel (moved from TASK-86)
- [x] #5 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the plugins panel: committed snapshots of the installed list in its healthy and error states, and an interaction test for enable/disable
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New module crates/sub-ui/src/plugins_panel.rs: PluginRow (id, name, version, location, directory, worlds, capabilities, enabled, dev) + LoadStatus (Disabled/NotLoaded/Loaded/Failed) built from sub_plugin::registry::Scan plus sub_plugin::dev::ReloadStatus rows, so scan LoadFailures and hot-reload ReloadErrors both land in the same list.
2. PluginsPanel: open flag, sync(scan, statuses), rows(), failures(), selection; show() as a window like DiagnosticsPanel/HistoryPanel and ui() for embedding. Errors (scan failures and reload errors) render inline under their row with the stable code, message and details.
3. PluginAction { Enable, Disable, Remove, OpenFolder } raised by the panel, never applied by it; PluginAction::perform(&PluginRegistry) routes enable/disable/remove through the existing registry API and open-folder through the platform opener, reporting ui.plugin_folder_unopenable on failure.
4. Export from lib.rs, add the new error code to crate::codes.
5. Tests: unit tests in the module for row building and status/error mapping; crates/sub-ui/tests/plugins_panel.rs on the shared harness with committed snapshots of the healthy and error lists plus an interaction test that clicks Disable/Enable over a real PluginRegistry in a temp dir and asserts the state file changed.
6. Verify with cargo fmt --check, clippy -D warnings and cargo test -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented as crates/sub-ui/src/plugins_panel.rs: PluginRow folds an InstalledPlugin's manifest (version, worlds, capabilities) together with the DevHost's ReloadStatus into a LoadStatus of Disabled / NotLoaded / Loaded{generation,commands,effects,tools} / Failed{generation,error}. PluginsPanel::sync(&Scan, &[ReloadStatus]) builds the list; scan LoadFailures become rows of their own so a directory that would not parse is visible beside the plugins. Both kinds of failure render inline under their row: message, stable code and every error detail (including the 'hint' the sub-plugin error catalogue fills in), which is the same path a --dev hot-reload failure takes, since a hot reload reports through ReloadStatus.error.

The panel mutates nothing. Each click raises a PluginAction (Enable/Disable/Remove/OpenFolder) that the caller performs with PluginAction::perform(&PluginRegistry), so the editor goes through the same registry calls as plugin.enable/plugin.disable/plugin.remove on the Command API. Opening a folder shells out to xdg-open/open/explorer and reports ui.plugin_folder_unopenable (new code in sub_ui::codes) when it cannot start.

Not wired into app.rs: the app does not yet own a PluginRegistry or a DevHost (the existing Plugins menu module is likewise unwired), so wiring is left to whichever task gives the app a plugin host. The panel is a window like DiagnosticsPanel/HistoryPanel, not a sixth dock panel, so dock layout files and their snapshots are untouched.

Verification (all in this worktree): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings exit 0 (plus cargo clippy -p sub-ui --all-targets after the last test was added); cargo test -p sub-ui green across every target, including 6 tests in crates/sub-ui/tests/plugins_panel.rs and 9 unit tests plus the module doctest in crates/sub-ui/src/plugins_panel.rs.

AC evidence: (1) plugins_panel_installed.png shows both rows with version, worlds, capabilities, install location and load status, and the unit tests pin worlds_label/capabilities_label/title/LoadStatus. (2) clicking_disable_and_enable_switches_the_plugin_in_the_registry drives Disable then Enable by accessibility label over a real PluginRegistry in a temp dir and asserts plugins.json changed both ways; removing_a_plugin_deletes_its_directory proves Remove; clicking_open_folder_raises_the_plugins_own_directory proves the Open folder button raises the action with the plugin's directory - actually starting the platform file manager is not exercised headlessly, and is the one part of AC 2 taken on the strength of a three-line std::process::Command call rather than a test. (3) and (4) plugins_panel_errors.png shows a failed --dev hot reload inline (message, plugin.load_failed, the hint detail) above an unscannable directory's own row, and a_failed_reload_keeps_the_row_and_carries_the_error pins the mapping.

Snapshots were recorded here with UPDATE_SNAPSHOTS=1 on the llvmpipe software adapter, the same path CI uses.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the plugins panel to sub-ui: crates/sub-ui/src/plugins_panel.rs folds a PluginRegistry scan and the DevHost's ReloadStatus rows into one PluginRow per installed plugin (version, worlds, capabilities, install location, dev flag and a Disabled/NotLoaded/Loaded/Failed load status), renders scan failures and reload errors inline with their stable code and details, and raises Enable/Disable/Remove/OpenFolder actions the caller performs through PluginAction::perform on the same registry the CLI and Command API use. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-ui, including two committed egui_kittest snapshots (healthy and error lists) and interaction tests that click Disable, Enable, Remove and Open folder on the shared harness over a real registry in a temp directory.
<!-- SECTION:FINAL_SUMMARY:END -->
