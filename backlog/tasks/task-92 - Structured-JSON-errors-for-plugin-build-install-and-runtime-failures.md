---
id: TASK-92
title: 'Structured JSON errors for plugin build, install and runtime failures'
status: Done
assignee:
  - '@opus-task-92'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:46'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-86
references:
  - docs/PLAN.md
priority: medium
ordinal: 113000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Errors are the agent's main feedback signal (§6.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 All plugin failures surface as SubError JSON with code, message, WIT type or function name, and a hint
- [x] #2 Manifest, capability and reload errors have distinct codes documented in the SDK
- [x] #3 CLI prints them as JSON with --json
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub_plugin::errors: a catalogue mapping every plugin.* code to the WIT type or function it belongs to and a one-line actionable hint, plus a PluginErrorExt trait (at_wit, explained) and the 'wit'/'hint' detail keys.
2. Enrich at the boundaries every plugin failure crosses: the WIT error record a plugin sees, LoadFailure and ReloadError JSON rows, the plugin.* Command API methods in registry and dev, and the CLI plugin subcommand.
3. Document the codes in the SDK: subordinate_sdk::errors with the code strings, their WIT item and hint, grouped so manifest, capability and reload codes are distinct and findable; keep them in step with a parity test in sub-plugin.
4. CLI: print SubError JSON honouring --json/--pretty/--compact on stderr.
5. Tests: catalogue completeness and well-formedness, enrichment at each boundary, SDK parity, CLI JSON output. Run fmt, clippy -D warnings and the touched crates' tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented as a single error catalogue rather than edits at ~55 construction sites.

sub_plugin::errors is the table: one row per code in sub_plugin::codes, each naming the WIT type or function the failure belongs to (subordinate:plugin/<interface-or-world>.<item>) and a one-line hint. errors::explain adds them as the 'wit' and 'hint' details without overwriting either, so a call site that knows a more precise WIT item can say so first with PluginErrorExt::at_wit. explain is idempotent and leaves non-plugin codes (core.*, command.*) untouched.

Applied at the points that report a plugin failure rather than at the JSON surfaces alone: Manifest::read_file, PluginRegistry::scan/set_enabled/remove, dev::install, DevHost::reload, LoadFailure::new, ReloadError::from, the WIT error record a plugin sees, and the CLI plugin subcommand. The plugin.* Command API methods and the MCP bridge inherit it from those.

subordinate_sdk::errors republishes the same table to plugin authors as &str code constants with their meanings, grouped in the module docs into manifest, capability/approval, install-list-reload, load and runtime-limit sections. The SDK is MIT/Apache and cannot depend on sub-core, so the table is duplicated as plain strings and kept honest by a parity test (sub-plugin dev-depends on the SDK for tests only).

CLI: a SubError now prints as JSON on stderr honouring --json/--pretty/--compact like a success does, instead of always compact; every 'eprintln(err.to_json())' site went through the same helper.

Validation (WSL, gstroot env sourced for sub-media deps): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p subordinate-sdk -p sub-plugin -p subordinate-cli green, 21 test targets, no warnings. New tests: crates/sub-plugin/src/errors.rs (7 unit tests: catalogue completeness against the declared constants, order, well-formedness, enrichment, idempotence), crates/sub-plugin/tests/errors.rs (6: broken manifest in a scan, failed reload and its recorded status, the WIT error record, a non-WIT failure, SDK parity, distinct manifest/capability/reload codes), bins/subordinate-cli/tests/plugin_errors.rs (4: code+hint in stderr JSON, indented vs --compact shape, a malformed id, an install that is not a plugin).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every plugin failure now leaves the host as a SubError carrying its stable plugin.* code, a message, the WIT type or function it belongs to when there is one, and a one-line hint, from a single catalogue in sub_plugin::errors applied at manifest reading, registry scan/enable/remove, install, reload, the LoadFailure and ReloadError rows, the WIT error record a plugin sees, and the CLI. The same table is republished to plugin authors as subordinate_sdk::errors, with manifest, capability and reload codes documented as distinct groups and a parity test keeping the two copies identical, and the CLI prints errors as JSON on stderr honouring --json/--compact. Verified by 17 new tests across the three crates plus cargo fmt --check and clippy --workspace --all-targets -D warnings, all green.
<!-- SECTION:FINAL_SUMMARY:END -->
