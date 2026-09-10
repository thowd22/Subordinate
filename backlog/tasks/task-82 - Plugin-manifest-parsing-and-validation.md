---
id: TASK-82
title: Plugin manifest parsing and validation
status: Done
assignee:
  - '@opus-task-82'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 00:50'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
references:
  - docs/PLAN.md
priority: high
ordinal: 103000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The manifest (§6.3) declares identity, worlds and capabilities.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 plugin.toml parsed into a Manifest struct: id (reverse-DNS), name, version (semver), api version, worlds, capabilities, mcp tool declarations
- [x] #2 Validation errors are SubError with field paths
- [x] #3 A JSON Schema for the manifest is generated and committed
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add manifest module to sub-plugin: Manifest/PluginSection/World/Capabilities/McpTool types with serde deny_unknown_fields, parsed from plugin.toml via the toml crate.
2. Validate identity (reverse-DNS id, semver version, api version compatibility, non-empty worlds, mcp tools only with the mcp-tools world, capability paths) collecting field paths; report failures as SubError with plugin.* codes and a field/fields detail.
3. Generate a JSON Schema of the manifest with schemars, commit it at docs/schema/plugin-manifest.json with a SUB_UPDATE_SCHEMA drift test and a README section, mirroring sub-command's schema export.
4. Unit tests for the plan §6.3 example manifest, each validation failure and schema stability; run fmt, clippy pedantic and cargo test -p sub-plugin.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-plugin/src/manifest.rs: Manifest { plugin, capabilities, mcp } parsed from plugin.toml with toml + serde deny_unknown_fields. PluginSection carries a validated reverse-DNS PluginId, a semver::Version, an ApiVersion (major.minor, checked against HOST_API_VERSION 0.1 where pre-1.0 minors are their own generation), the World enum from PLAN 6.2, plus optional description/authors. Capabilities are fs_read/fs_write/network/shaders — declaration only; grant and enforcement stay with TASK-83/84. McpSection holds [mcp.tools.<name>] entries with description and a schema path.

Parsing goes through a private RawManifest mirror whose scalars are strings, so a malformed id, version or api reports its own dotted field path instead of a TOML span. Errors: plugin.invalid_manifest_syntax (bad TOML, unknown key, wrong type; line/column details), plugin.invalid_manifest (rule broken; details field, problem, and fields listing every problem in manifest order), plugin.manifest_unreadable (I/O, with path). All are SubError; new codes are declared in sub_plugin::codes.

Added crates/sub-plugin/src/schema.rs generating the manifest JSON Schema from the same types, committed at docs/schema/plugin-manifest.json with a committed_schema_is_up_to_date drift test (SUB_UPDATE_SCHEMA=1 regenerates) and a docs/schema/README.md section, mirroring the sub-command and sub-model schema exports.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-plugin 33 unit + 3 integration + 4 doc tests pass, including the PLAN 6.3 example manifest parsed field for field.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
plugin.toml is now parsed and validated by sub_plugin::manifest into a Manifest struct covering identity (reverse-DNS id, name, semver version, major.minor api version), the WIT worlds, the requested capabilities and the declared MCP tools, with strict unknown-key rejection. Every failure is a SubError with a stable plugin.* code and the dotted field path of the offending key (all problems listed in a fields detail), and the manifest's JSON Schema is generated from the same types and committed at docs/schema/plugin-manifest.json behind a drift test. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-plugin (40 tests, all passing).
<!-- SECTION:FINAL_SUMMARY:END -->
