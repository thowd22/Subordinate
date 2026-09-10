---
id: TASK-81
title: 'WIT mcp-tools world: plugin-contributed MCP tools'
status: Done
assignee:
  - '@opus-task-81'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:54'
labels:
  - plugins
  - mcp
milestone: m-6
dependencies:
  - TASK-84
references:
  - docs/PLAN.md
priority: high
ordinal: 102000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Plugins extend the agent surface (§6.2, §7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 mcp-tools world exports tools() -> list<ToolDesc { name, description, json-schema }> and call(name, args-json) -> result-json
- [x] #2 Manifest declares the tools; the host validates arguments against the schema before calling
- [x] #3 Tools appear through the MCP bridge with the plugin id as a prefix
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-plugin::mcp: build a ToolCatalog straight from an installed plugin (manifest [mcp.tools.*] plus the schema file each entry names, read relative to the plugin directory), with a new plugin.tool_schema_unreadable code for a schema file that will not read.
2. sub-plugin::registry: a new query method plugin.tools listing every enabled plugin's contributed tools as the bridge should publish them (prefixed mcp name, dotted title, description, input schema, plugin id, local name), plus per-plugin failures so one bad schema does not hide the rest. Registered in register_methods and exported in the plugin-api schema document (regenerate docs/schema/plugin-api.json).
3. subordinate-mcp::tools: ToolSet gains plugin tools — extend_from_plugin_tools(Value) parses a plugin.tools result into rmcp Tools and a route table, plugin_route(name) maps a published name back to {plugin id, local tool}.
4. subordinate-mcp::bridge: tools/list asks the editor for plugin.tools on a blocking task and appends them (a failure logs and leaves the static tools), and tools/call routes a plugin tool to plugin.call_tool with {id, tool, arguments}; the listing's cache hints drop to session scope when plugin tools are present.
5. Tests: catalogue from a manifest directory; plugin.tools over a dispatcher with a temp plugin dir; ToolSet parsing, prefixing and reverse routing; bridge integration test that a plugin's tool appears in tools/list under its plugin-id prefix.
6. Verify cargo fmt --check, clippy --workspace --all-targets -D warnings, cargo test -p sub-plugin -p subordinate-mcp.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added the mcp-tools world to wit/subordinate-plugin.wit: a new mcp interface holding record tool-desc { name, description, json-schema }, and a world that imports command-api and exports tools() -> list<tool-desc> and call(name, args-json) -> result<string, error>. Host bindings are a second bindgen! in crates/sub-plugin/src/bindings.rs whose with map points subordinate:plugin/types and subordinate:plugin/command-api at the command world's expansion, so both worlds share one command_api::Host trait and one WitError.

New module crates/sub-plugin/src/mcp.rs is the host half: ToolDeclaration (a manifest [mcp.tools.*] entry) and ToolCatalog (per plugin). The catalogue compiles each declared JSON Schema on load, refuses a component whose tools() export names an undeclared tool, omits a declared one, or ships a schema that differs from the manifest's, and validates every call's arguments against the declared schema before dispatch, reporting all violations at once as JSON Pointers in the error's violations detail. Nine new stable plugin.* codes cover the failures. jsonschema 0.55 is a new dependency with default-features off: the defaults pull reqwest/rustls to resolve remote refs, and a plugin tool schema must be self-contained rather than fetched at call time.

Naming: an MCP tool name is the plugin id with dots turned into underscores, then an underscore, then the plugin-local name (com.example.silence-cutter + cut_silence -> com_example_silence-cutter_cut_silence). That reuses the dot-to-underscore rule subordinate-mcp already applies to Command API method names and stays inside the [A-Za-z0-9_-] set MCP clients accept; ToolCatalog::local_name maps a prefixed name back so the bridge routes a call without guessing. AC #3 is only half provable here: the prefixing contract and the reverse lookup are implemented and tested, but tools cannot actually appear through the running MCP bridge until the wasmtime plugin host (TASK-84) can instantiate a plugin and TASK-96 wires plugin catalogues into subordinate-mcp's tool list. Left unchecked rather than claimed.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (GStreamer prefix env exported for sub-media); cargo test -p sub-plugin 17 unit + 4 integration + 3 doc tests pass, including the_mcp_tools_world_links_against_the_same_host, which links the world into a wasmtime Linker with every import satisfied by the same TestHost the command world uses. cargo test -p spike-wasm-command-world (7 tests) also passes, which rebuilds the wasm32-wasip2 guest components against the edited WIT package, so the new interface and world parse under wit-bindgen as well as wasmtime bindgen. AC #1 and #2 checked on that evidence; AC #3 left unchecked (see the note above).

2026-09-10: requeued; the wasmtime host (TASK-84), registry (TASK-85) and MCP bridge (TASK-93) now exist, so criterion 3 (plugin tools listed through the bridge with the plugin id prefix) can be completed.

2026-09-10 (criterion 3): plugin-contributed tools now reach the MCP bridge. Host side: ToolCatalog::from_manifest builds a plugin's catalogue straight from its manifest's [mcp.tools.*] entries and the schema file each one names (new stable code plugin.tool_schema_unreadable), ToolCatalog::published gives the wire shape, and registry::contributed_tools walks a scan's enabled plugins into a ToolListing (per-plugin failures, so one unreadable schema hides only its own plugin, and a published-name collision is reported rather than shadowing). A fifth query method, plugin.tools, is registered beside plugin.list/enable/disable/remove and exported in docs/schema/plugin-api.json (regenerated with SUB_UPDATE_SCHEMA=1).

Bridge side: subordinate_mcp::tools::PluginTools parses a plugin.tools answer into rmcp Tools plus a route table (mcp.plugin_tools_invalid for an answer that is not a listing). Bridge::published_tools refreshes that from the editor on every tools/list and appends the plugin tools to the compiled-in ones; a listing carrying plugin tools drops to a private, 30s cache hint because which plugins are installed is that editor's business, not the build's. A call naming a tool the compiled-in schemas do not describe is routed by plugin id to plugin.call_tool rather than refused, so TASK-96 only has to serve that method; an editor that does not serve plugin.tools yet keeps the previous answer instead of failing the listing.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (GStreamer prefix env exported); cargo test -p sub-plugin (134 unit + integration + doc tests), -p subordinate-mcp (32 unit, 5 bridge, plus the rest) and -p subordinate-cli all pass. The bridge integration test a_plugins_tools_are_offered_under_its_id_and_route_back_to_it installs a plugin declaring [mcp.tools.cut_silence], serves the registry over a real local socket, and asserts the bridge offers com_example_demo_cut_silence with the manifest's description and schema, routes a call to it back to that plugin, and drops it again when the plugin is disabled.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The mcp-tools WIT world and its host-side catalogue (criteria 1 and 2) are joined by the bridge path (criterion 3): plugin manifests' [mcp.tools.*] declarations become a ToolListing served by the new plugin.tools Command API method, and subordinate-mcp publishes each one as an MCP tool named for its plugin id, routing calls back to the declaring plugin. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test -p sub-plugin -p subordinate-mcp -p subordinate-cli, including an integration test that lists a plugin's tool through the bridge over a real socket.
<!-- SECTION:FINAL_SUMMARY:END -->
