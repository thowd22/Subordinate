---
id: TASK-81
title: 'WIT mcp-tools world: plugin-contributed MCP tools'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:02'
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
- [ ] #3 Tools appear through the MCP bridge with the plugin id as a prefix
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add an `mcp` interface (record tool-desc { name, description, json-schema }) and a `mcp-tools` world to wit/subordinate-plugin.wit: imports command-api, exports tools() -> list<tool-desc> and call(name, args-json) -> result<string, error>.
2. Generate host bindings for the new world in sub-plugin (second bindgen! with `with` remapping so types and command-api stay the same Rust types as the command world).
3. Add a host-side tool catalogue in sub-plugin: ToolDeclaration (the manifest's [mcp.tools.*] entry: name, description, JSON Schema) and ToolCatalog keyed by plugin id, which (a) rejects undeclared or mismatched tools returned by the plugin's tools(), (b) validates call arguments against the declared JSON Schema before dispatch (jsonschema crate, default-features off so no remote ref resolution), (c) exposes the plugin-id-prefixed MCP tool names and maps a prefixed name back to the plugin-local tool.
4. New stable SubError codes under plugin.* for invalid plugin id, invalid tool name, invalid tool schema, undeclared tool and invalid tool arguments.
5. Tests: WIT world links and its exports are callable shapes; catalogue accepts/rejects; schema validation passes and fails with field paths in details; prefixing and reverse lookup round-trip.
6. Verify with cargo fmt --check, clippy --workspace --all-targets -D warnings and cargo test -p sub-plugin.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added the mcp-tools world to wit/subordinate-plugin.wit: a new mcp interface holding record tool-desc { name, description, json-schema }, and a world that imports command-api and exports tools() -> list<tool-desc> and call(name, args-json) -> result<string, error>. Host bindings are a second bindgen! in crates/sub-plugin/src/bindings.rs whose with map points subordinate:plugin/types and subordinate:plugin/command-api at the command world's expansion, so both worlds share one command_api::Host trait and one WitError.

New module crates/sub-plugin/src/mcp.rs is the host half: ToolDeclaration (a manifest [mcp.tools.*] entry) and ToolCatalog (per plugin). The catalogue compiles each declared JSON Schema on load, refuses a component whose tools() export names an undeclared tool, omits a declared one, or ships a schema that differs from the manifest's, and validates every call's arguments against the declared schema before dispatch, reporting all violations at once as JSON Pointers in the error's violations detail. Nine new stable plugin.* codes cover the failures. jsonschema 0.55 is a new dependency with default-features off: the defaults pull reqwest/rustls to resolve remote refs, and a plugin tool schema must be self-contained rather than fetched at call time.

Naming: an MCP tool name is the plugin id with dots turned into underscores, then an underscore, then the plugin-local name (com.example.silence-cutter + cut_silence -> com_example_silence-cutter_cut_silence). That reuses the dot-to-underscore rule subordinate-mcp already applies to Command API method names and stays inside the [A-Za-z0-9_-] set MCP clients accept; ToolCatalog::local_name maps a prefixed name back so the bridge routes a call without guessing. AC #3 is only half provable here: the prefixing contract and the reverse lookup are implemented and tested, but tools cannot actually appear through the running MCP bridge until the wasmtime plugin host (TASK-84) can instantiate a plugin and TASK-96 wires plugin catalogues into subordinate-mcp's tool list. Left unchecked rather than claimed.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (GStreamer prefix env exported for sub-media); cargo test -p sub-plugin 17 unit + 4 integration + 3 doc tests pass, including the_mcp_tools_world_links_against_the_same_host, which links the world into a wasmtime Linker with every import satisfied by the same TestHost the command world uses. cargo test -p spike-wasm-command-world (7 tests) also passes, which rebuilds the wasm32-wasip2 guest components against the edited WIT package, so the new interface and world parse under wit-bindgen as well as wasmtime bindgen. AC #1 and #2 checked on that evidence; AC #3 left unchecked (see the note above).

2026-09-10: requeued; the wasmtime host (TASK-84), registry (TASK-85) and MCP bridge (TASK-93) now exist, so criterion 3 (plugin tools listed through the bridge with the plugin id prefix) can be completed.
<!-- SECTION:NOTES:END -->
