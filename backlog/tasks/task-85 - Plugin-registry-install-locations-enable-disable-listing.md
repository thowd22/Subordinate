---
id: TASK-85
title: 'Plugin registry: install locations, enable/disable, listing'
status: Done
assignee:
  - '@opus-task-85'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 06:13'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-84
references:
  - docs/PLAN.md
priority: high
ordinal: 106000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users and agents need to see and manage installed plugins.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Plugins load from a user plugin dir and a project-local plugin dir; project-local wins on id conflict with a warning
- [x] #2 plugin list, enable, disable, remove via CLI and MCP
- [x] #3 Load failures are reported per plugin without aborting startup
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-plugin: new `registry` module. `PluginDirs` (per-user data dir + optional project-local dir, platform defaults mirroring endpoint::runtime_directory), `InstallLocation` {User, Project} with project-local winning on id conflict and a recorded warning, `PluginRegistry::scan` discovering every subdirectory holding a plugin.toml, parsing manifests and recording a per-plugin `LoadFailure` instead of aborting.
2. Enable state: a small JSON file (schema_version + disabled ids) in the user plugin dir; `set_enabled` and `remove` rewrite it. Registry is stateless across calls so two processes agree.
3. New stable error codes: plugin.not_installed, plugin.plugins_dir_unreadable, plugin.invalid_registry_state, plugin.registry_state_unreadable, plugin.registry_state_unwritable, plugin.remove_failed.
4. Command API surface: register plugin.list/enable/disable/remove as query methods on a Dispatcher (Dispatcher::register), exported as a committed JSON Schema at docs/schema/plugin-api.json with an up-to-date test.
5. MCP: subordinate-mcp's ToolSet gains the committed plugin-api document, so plugin_list/plugin_enable/plugin_disable/plugin_remove are tools forwarded over the socket like any other method. subordinate-cli serve registers the methods.
6. CLI: `subordinate-cli plugin list|enable|disable|remove` acting on the registry directly, JSON out like the other subcommands.
7. Tests: discovery precedence + warning, per-plugin failure isolation, enable/disable/remove round trips, dispatcher methods, tool set, CLI parsing. Then fmt, clippy -D warnings, cargo test.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in three layers.

**Registry (crates/sub-plugin/src/registry.rs).** `PluginDirs` holds the per-user directory (platform data dir: $XDG_DATA_HOME/subordinate/plugins, ~/Library/Application Support/Subordinate/plugins, %APPDATA%\\Subordinate\\plugins) and an optional project-local one, `.subordinate/plugins` beside the project file. `PluginRegistry::scan` walks both in precedence order, parses every `plugin.toml`, and returns a `Scan` of loaded plugins, per-directory `LoadFailure` rows and `Shadowed` rows; project-local wins an id conflict and the losing copy is both recorded and logged with tracing::warn. A directory holding no manifest is not a plugin and is passed over silently. Enable state lives in `plugins.json` in the user directory (schema_version + the ids switched off); the registry caches nothing, so the GUI, CLI and MCP bridge agree without a lock. New stable codes: plugin.not_installed, plugin.plugin_dir_unreadable, plugin.plugin_dir_unavailable, plugin.invalid_registry_state, plugin.registry_state_unreadable, plugin.registry_state_unwritable, plugin.remove_failed.

**Command API + MCP.** `registry::register_methods` puts plugin.list/enable/disable/remove on a Dispatcher through the existing `Dispatcher::register` hook; they are queries, not undoable Commands, because they change what the host loads and never the project. Their JSON Schema is exported by `registry::schema` and committed at docs/schema/plugin-api.json with the same up-to-date test the other schemas use. subordinate-mcp's ToolSet gained `extend_from_schema` and compiles that document in beside command-api.json, so plugin_list/plugin_enable/plugin_disable/plugin_remove are ordinary MCP tools forwarded over the socket — no new dependency and no hand-written tool list. `subordinate-cli serve` registers the methods (with an optional --plugin-dir override); a machine with no data directory is served without them rather than not served.

**CLI.** `subordinate-cli plugin list|enable|disable|remove` (bins/subordinate-cli/src/plugin.rs) acts on the registry directly, so it works with no editor running; --dir overrides the user directory and --project adds a project's own. Output is JSON like every other subcommand; the listing carries user_dir and project_dir.

Deliberately out of scope: install/uninstall from a .wasm (TASK-86's --dev install), hot reload, and the plugins panel (TASK-98). plugin.new/install/reload/test as MCP tools stay TASK-96's.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test --workspace passes with no failures (run with the local GStreamer prefix sourced for sub-media). Manual smoke: subordinate-cli plugin list on an empty directory, then install a fixture manifest and run disable, list and a remove of an absent id, which exits 1 with the plugin.not_installed SubError on stderr.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the plugin registry: sub-plugin's new `registry` module discovers plugins in a per-user data directory and a project-local `.subordinate/plugins`, with project-local winning an id conflict (the hidden copy is reported as Shadowed and logged), reports a bad `plugin.toml` as one LoadFailure row while every other plugin still loads, and keeps enable/disable state in a `plugins.json` it re-reads on every call. The same four operations reach users and agents both ways: `subordinate-cli plugin list|enable|disable|remove` works against the directories with no editor running, and `registry::register_methods` puts plugin.list/enable/disable/remove on the Command API (schema committed at docs/schema/plugin-api.json) so subordinate-mcp offers them as plugin_* MCP tools. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, cargo test --workspace (new: 14 registry tests, 4 CLI plugin tests, 3 CLI parser tests, 2 ToolSet tests, and a bridge integration test that lists, disables and removes a plugin through MCP end to end), plus a manual CLI run of list/disable/remove.
<!-- SECTION:FINAL_SUMMARY:END -->
