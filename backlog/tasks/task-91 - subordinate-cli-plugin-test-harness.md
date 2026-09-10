---
id: TASK-91
title: subordinate-cli plugin test harness
status: Done
assignee:
  - '@opus-task-91'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 17:07'
labels:
  - plugins
  - cli
  - test
milestone: m-6
dependencies:
  - TASK-90
references:
  - docs/PLAN.md
priority: high
ordinal: 112000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Agents need a fast, headless way to prove a plugin works (§6.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 plugin test <id> loads the plugin headlessly, runs its declared tests against the fixture project and reports structured JSON results
- [x] #2 Effect plugins are tested by rendering a frame; command plugins by asserting project state after run
- [x] #3 Non-zero exit and readable output on failure
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub_plugin::harness: a headless plugin host (WASI ctx, StoreLimits, command-api served by a real EngineHandle + Dispatcher, analysis-host progress/cancel) and per-world checks returning a serialisable report of pass/fail/skip checks.
2. Command world: instantiate, call run inside an engine undo group, record project state before and after (revision, counts, JSON digest) and assert the run is undoable; effect world: describe, lift into sub_render::EffectDesc and render one frame through the compositor on a test picture (render feature), skipping with a reason where the machine has no wgpu adapter; analyzer world: analyze over the fixture media, lifting findings; mcp-tools world: tools checked against plugin.toml and each declared tool called with schema-valid arguments.
3. Wire 'subordinate-cli plugin test <id> [--fixture <file>] [--dir] [--project]': resolve the installed plugin from the registry, pick its fixture project (--fixture, the dev source tree's fixture/fixture.sub, the plugin dir's, or a temporary scaffolded one), spawn the engine and dispatcher, run the harness, print the structured JSON report and exit non-zero with a readable stderr summary when a check fails.
4. Tests: two new wasm guests in sub-plugin (a command plugin that edits the project through the Command API, an effect plugin declaring WGSL) exercised by tests/harness.rs; CLI tests for argument parsing, plugin resolution, fixture selection, the report shape and the failure path.
5. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, cargo test for sub-plugin and subordinate-cli.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented as three pieces.

sub_plugin::harness (new): the headless host and the checks. HarnessHost serves command-api out of a live EngineHandle and the same sub_command::Dispatcher the socket serves — run-command and query dispatch for real, the accessors answer from the engine snapshot, log lines are captured — with a WASI context that grants nothing. Harness::run compiles the component once (a load failure is a failed 'component_loads' check, not an Err, so the report is still printed) and then runs the checks each declared world calls for: command = instantiate, run inside an engine undo group, answer must be JSON, project state before/after recorded, and the change must undo exactly (the JSON of the project is compared, not counts); effect = describe, lift into sub_render::EffectDesc, render a frame; analyzer = analyze over the fixture's media plus lifting the findings; mcp-tools = ToolCatalog::from_manifest, accept_exports against the component's tools, then one call per tool with schema-valid arguments (skipped, with the schema's own complaint, when {} is rejected). Every check is pass/fail/skip with a message, details and the structured SubError (code, WIT item, hint) behind a failure; TestReport::ok is false when any check failed or none passed.

sub_render::probe (new): probe_effect renders one 16x16 flat-grey frame through the ordinary Compositor with the declaration bound at its defaults, once without the effect and once with it, and reports whether it applied, the compiler's message when it did not, and both pixels. This keeps wgpu out of sub-plugin: only the render feature's conversion is needed there.

subordinate-cli plugin test <id> [--fixture <file>] [--args <json>]: resolves the installed plugin (refreshing a dev install's copies), picks the fixture (--fixture, the dev source tree's fixture/fixture.sub, the installed one, else a starter project written to a scratch directory that is deleted afterwards), opens it, spawns the engine and dispatcher, runs the harness and prints the report with the fixture, component, directory and location alongside. main::plugin_report exits non-zero and puts the one-line summary on stderr whenever a plugin answer carries ok:false.

Two new wasm test guests: 'editor' (command world, adds a track through run-command) and 'tinter' (effect world, one float parameter and a WGSL fragment entry), both built by sub-plugin's existing build script.

Validation on this machine: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -D warnings clean; cargo test -p sub-plugin -p sub-render -p subordinate-cli all green, including the five new tests/harness.rs cases (a command plugin's edit is seen and undone, a fuel-exhausted run fails with plugin.fuel_exhausted and its hint, a non-component fails only the load check, an effect renders a frame, a world the component does not export fails to instantiate) and two new probe tests. End to end through the built CLI: 'plugin install <guest> --dev' then 'plugin test com.example.editor' printed 6 passing checks (tracks 2 -> 3, undo restored it) and exited 0; 'plugin test com.example.tinter' rendered the frame on llvmpipe (input 128 -> output 66 with amount 0.25) and exited 0; a plugin whose manifest declares a world its component does not export printed the failed check with plugin.instantiate_failed and exited 1 with 'com.example.broken: 1 passed, 1 failed, 0 skipped' on stderr.

Note: the CLI now enables sub-plugin's render feature, so it links wgpu — that is what makes 'tested by rendering a frame' true in the same process; the MCP server and other headless consumers are unaffected.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
subordinate-cli plugin test <id> now loads an installed plugin headlessly, runs the checks its declared worlds call for against a fixture project and prints one structured JSON report: a command plugin is run through the real Command API and its effect on the project is recorded and undone, an effect plugin has a frame rendered with its shader, an analyzer's findings are lifted into the model and an mcp-tools plugin's tools are checked against its manifest and called. Failures are checks, not crashes — each carries its stable code, WIT item and hint — and the CLI exits non-zero with a one-line summary on stderr when any check fails. Verified by five new sub-plugin harness tests over real wasm guests, two sub-render probe tests, and an end-to-end run of the built CLI (6 checks passing on a command plugin, a frame rendered on llvmpipe for an effect plugin, exit 1 on a mismatched one); fmt, workspace clippy -D warnings and the tests of every touched crate are clean.
<!-- SECTION:FINAL_SUMMARY:END -->
