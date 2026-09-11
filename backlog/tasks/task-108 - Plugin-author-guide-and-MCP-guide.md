---
id: TASK-108
title: Plugin author guide and MCP guide
status: Done
assignee:
  - '@opus-task-108'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 00:57'
labels:
  - docs
milestone: m-7
dependencies:
  - TASK-102
references:
  - docs/PLAN.md
priority: high
ordinal: 129000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The plugin system is the product; its documentation must be excellent for both humans and agents.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 docs/plugin-guide.md covers each world, the manifest, capabilities, the dev loop and testing
- [x] #2 docs/mcp-guide.md covers .mcp.json setup, every tool family and the agent runbook
- [x] #3 Both link to the first-party plugins as worked examples
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Survey the material the guides must be true to: wit/README.md and wit/subordinate-plugin.wit (worlds), sub-plugin manifest/capability/registry/dev/harness/runtime/mcp/menu/interchange module docs, the SDK, the first-party plugins, the CLI plugin subcommands, docs/schema/*.json and the MCP bridge (tools, resources, env).
2. Write docs/plugin-guide.md: what a plugin is, every world (command/commands, effect, effect-cpu, audio-effect, importer, exporter, analyzer, mcp-tools, panel post-MVP) with its exports and its worked example, plugin.toml field by field, the capability model and approval, the dev loop (new/build/install --dev/hot reload/test), the headless harness and what it checks per world, limits and error codes.
3. Write docs/mcp-guide.md: what the bridge is, .mcp.json setup and the environment table, every tool family from the two committed schemas (project/sequence/track/clip/transition/marker/media/bin/edit/history/events/system plus plugin.*), plugin-contributed tools and their naming rule, the resources, error handling, and the agent runbook.
4. Both link the first-party plugins (color, gain, cut-silence, otio) and the runbook as worked examples.
5. Add docs-consistency tests in the existing style (bins/subordinate-mcp/tests/mcp_json.rs, runbook.rs): a test that every tool name the MCP guide names is served by the bridge and every env var it documents is read, and a test that the plugin guide names every World and every capability key and that its manifest snippets parse.
6. Link the two guides from README.md and docs/DEVELOPMENT.md.
7. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test for sub-plugin and subordinate-mcp.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Wrote docs/plugin-guide.md and docs/mcp-guide.md, and linked both from README.md and docs/DEVELOPMENT.md.

docs/plugin-guide.md: what a plugin is (one component, the three rules — never holds the model, exact rational time, stable error codes), a table of all nine hostable worlds plus `panel` with each one's exports and its worked example, then a section per world (command/commands, effect/effect-cpu, audio-effect, importer/exporter, analyzer, mcp-tools); plugin.toml field by field with the strict-parsing rule; the capability model (the four keys, $PROJECT/$PLUGIN_DATA roots, expansion/resolution/gating, approval re-prompted on a manifest digest change); the developer loop (plugin new/build/install --dev/hot reload/test) with the real CLI flags, install locations and precedence; the headless harness and what it checks per world; fuel, epoch and memory limits and their codes; the error catalogue; licensing.

docs/mcp-guide.md: what the bridge is and why one tool is one Command API method, .mcp.json setup with the environment table, agent conventions (rational time, undo groups, read resources first, errors are data), every tool family — project/history/system reads, clip/track/sequence/transition/marker edits, media and bins, edit undo/redo/grouping, events, the eleven plugin.* tools, and plugin-contributed tools with their namespacing rule — the three resources and their caching/subscription behaviour, the error codes worth recognising, and the agent runbook with its preconditions.

Tests, in the style of the existing docs-consistency tests (bins/subordinate-mcp/tests/mcp_json.rs, runbook.rs):
- bins/subordinate-mcp/tests/mcp_guide.rs — the guide names every tool ToolSet::committed() serves (67 of them), names no tool-shaped name the bridge does not serve, documents exactly the five environment variables the bridge reads, and links the worked-example paths, which must exist.
- crates/sub-plugin/tests/plugin_guide.rs — the guide names every World::ALL variant and every capability key, its plugin.toml block parses with Manifest::parse, every plugin.* code it names is in errors::CATALOGUE, and its worked-example links exist.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the local GStreamer env exported); cargo test -p sub-plugin -p subordinate-mcp all green, including the two new test files (5 and 4 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the two guides the plugin system needs to be usable by humans and agents: docs/plugin-guide.md (every WIT world with its exports and worked example, plugin.toml field by field, the capability model and approval, the scaffold/build/dev-install/hot-reload/test loop, the headless harness, the runtime limits and the error catalogue) and docs/mcp-guide.md (.mcp.json setup and the environment table, every tool family generated from the two committed schemas, plugin-contributed tools and their namespacing, the three resources, the error codes and the agent runbook). Both link the first-party plugins — color, gain, cut-silence, otio — and docs/agent-runbook.md as worked examples, and both are linked from README.md and docs/DEVELOPMENT.md. Two new docs-consistency tests keep them true to the build: bins/subordinate-mcp/tests/mcp_guide.rs asserts the MCP guide names every tool the bridge serves, no tool it does not, and exactly the environment variables it reads; crates/sub-plugin/tests/plugin_guide.rs asserts the plugin guide names every World and capability key, that its plugin.toml parses with Manifest::parse, and that every plugin.* code it cites is in errors::CATALOGUE. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, and cargo test -p sub-plugin -p subordinate-mcp, all clean.
<!-- SECTION:FINAL_SUMMARY:END -->
