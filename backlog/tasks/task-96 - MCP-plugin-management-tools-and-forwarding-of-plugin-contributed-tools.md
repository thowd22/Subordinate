---
id: TASK-96
title: MCP plugin management tools and forwarding of plugin-contributed tools
status: Done
assignee:
  - '@opus-task-96'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 19:17'
labels:
  - mcp
  - plugins
milestone: m-6
dependencies:
  - TASK-93
  - TASK-91
  - TASK-81
references:
  - docs/PLAN.md
priority: high
ordinal: 117000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Closes the agent loop: the agent installs and tests its own plugin from the conversation (§6.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 plugin.new, plugin.install, plugin.reload, plugin.test, plugin.list exposed as MCP tools
- [x] #2 Tools from mcp-tools plugins are listed with the plugin id prefix and list_changed is sent on reload
- [x] #3 Integration test installs a plugin and calls its tool through MCP
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-plugin: new `authoring` module with plugin.new and plugin.test Command API methods, served by handler closures the host supplies (the scaffold and the test harness live in subordinate-cli), plus their JSON Schema entries.
2. sub-plugin/dev.rs: a ToolCaller alongside the Loader, DevHost::call_tool validating arguments against the plugin's ToolCatalog, and the plugin.call_tool method the MCP bridge already routes to.
3. sub-plugin/harness.rs: Harness::call_tool — instantiate one plugin in the mcp-tools world against the live engine and dispatcher and call one tool — and dev::harness_tool_caller wrapping it.
4. Regenerate docs/schema/plugin-api.json so plugin.new, plugin.test and plugin.call_tool become MCP tools beside plugin.list, plugin.install and plugin.reload.
5. subordinate-cli serve: register the authoring methods and a real tool caller over a plugin-facing dispatcher (the engine's methods only, so a plugin cannot manage plugins).
6. subordinate-mcp bridge: declare tools list_changed, and notify the peer after a plugin management tool call that changes the published tool set (install, reload, enable, disable, remove).
7. New wasm32-wasip2 test guest exporting the mcp-tools world; expose its path from sub-plugin under the test-guests feature.
8. Integration test in bins/subordinate-mcp/tests: install that guest through plugin_install over MCP and call its tool by its prefixed name.
9. cargo fmt, clippy -D warnings, tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
AC1: plugin.install, plugin.reload and plugin.list were already exported; added plugin.new and plugin.test as Command API methods in a new sub_plugin::authoring module (wire types, names and schema there; the scaffolder and the harness runner stay in subordinate-cli and are supplied as handlers by serve). All five are now in docs/schema/plugin-api.json, which the bridge compiles in, so they become plugin_new, plugin_install, plugin_reload, plugin_test and plugin_list. Proved by registry::schema::tests (method-name order) and tools::tests, plus the regenerated committed document.

AC2: the id prefix was already in place from TASK-93. Added the missing half — plugin.call_tool is now actually served (sub_plugin::dev::DevHost::call_tool plus a ToolCaller the host supplies; Harness::call_tool instantiates the mcp-tools world against the live engine), the bridge declares tools listChanged, and after a plugin_install/reload/remove/enable/disable it re-reads plugin.tools and sends notifications/tools/list_changed when the listing moved. PluginTools now keeps a digest of the listing so a reload that only changes a description is still announced.

AC3: bins/subordinate-mcp/tests/plugin_tools.rs installs a real wasm32-wasip2 component over MCP (plugin_install), finds its tool published as com_example_toolbox_create_bin, calls it, and asserts the bin it created is undoable through edit_undo; bad arguments come back as plugin.invalid_tool_arguments. A new tests/guests/toolbox guest exports the mcp-tools world, and sub_plugin::guests (test-guests feature) hands its path to downstream test crates.

Design notes: a plugin tool runs against a dispatcher holding only the engine's methods, so a plugin edits the project but cannot install, reload or remove plugins. serve now loads with manifest_tools_loader rather than compile_only_loader so tools are listed and validated without entering the component; the component is entered only by plugin.call_tool. No new error codes: unknown tool is plugin.undeclared_tool, bad arguments plugin.invalid_tool_arguments, a host that runs no plugins core.unimplemented.

Verified: cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, and cargo test -p sub-plugin -p subordinate-cli -p subordinate-mcp all pass (GStreamer env sourced).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Closed the agent plugin loop over MCP. plugin.new and plugin.test joined plugin.install, plugin.reload and plugin.list on the Command API (new sub_plugin::authoring module, handlers supplied by subordinate-cli serve), so all five are exported in docs/schema/plugin-api.json and published by the bridge. plugin.call_tool is now served for real — DevHost validates a call against the manifest's schema and runs the component in the mcp-tools world against a plugin-facing dispatcher that holds only the engine's methods — and the bridge declares tools listChanged and announces it whenever a plugin install, reload, remove, enable or disable moves the listing. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, and cargo test over sub-plugin, subordinate-cli and subordinate-mcp, including a new end-to-end test that installs a real wasm32-wasip2 plugin over MCP, calls its tool and undoes the edit it made, and a stdio test that waits for notifications/tools/list_changed after an install and a reload.
<!-- SECTION:FINAL_SUMMARY:END -->
