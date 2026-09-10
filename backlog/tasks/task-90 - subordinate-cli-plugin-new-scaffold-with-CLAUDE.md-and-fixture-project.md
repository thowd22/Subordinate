---
id: TASK-90
title: subordinate-cli plugin new scaffold with CLAUDE.md and fixture project
status: Done
assignee:
  - '@opus-task-90'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 13:51'
labels:
  - plugins
  - cli
milestone: m-6
dependencies:
  - TASK-89
  - TASK-86
references:
  - docs/PLAN.md
priority: high
ordinal: 111000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Agent-first principle (§6.1): a plugin starts from a single command.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 plugin new --world <world> <name> generates a cargo-component project, plugin.toml, a CLAUDE.md describing the world interface, host imports and test contract, plus a fixture project
- [x] #2 Generated project builds and installs with no edits
- [x] #3 Templates exist for command, effect, analyzer and mcp-tools worlds
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a scaffold module to subordinate-cli: 'plugin new --world <world> <name> [--dir <dir>] [--id <id>] [--sdk-path <path>] [--force]'.
2. Generate per-world templates (command, effect, analyzer, mcp-tools): Cargo.toml (cdylib, own workspace, subordinate-sdk with the world feature), src/lib.rs implementing that world's Guest trait, plugin.toml, .gitignore, and for mcp-tools a schemas/<tool>.json.
3. Generate CLAUDE.md per world: world interface, host imports, capabilities, build/install/test contract.
4. Generate a fixture project (fixture/fixture.subproj) with the same starter sequence and tracks 'subordinate-cli new' writes.
5. Validate the generated manifest with sub_plugin::manifest::Manifest::parse before writing, so a scaffold can never emit a manifest the host rejects. Report JSON listing every file written.
6. Wire into the argument parser and USAGE; unit tests for parsing and generation, an integration test over the built binary, and an opt-in test that actually cargo-builds and installs the scaffold (needs the wasm32-wasip2 target and the crates.io cache).
7. Verify with cargo fmt, clippy pedantic and cargo test -p subordinate-cli.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented `subordinate-cli plugin new --world <world> <name> [--output <dir>] [--id <id>] [--sdk-path <dir>] [--force]` in a new bins/subordinate-cli/src/scaffold.rs, dispatched through plugin::Action::New so it answers before any plugin directory is resolved (scaffolding works on a machine with nothing installed).

Each scaffold is a whole crate: Cargo.toml (cdylib, edition 2024, rust-version 1.95, subordinate-sdk with exactly the world's feature, its own [workspace] so it never joins the host workspace), src/lib.rs implementing that world's Guest trait, plugin.toml, CLAUDE.md, .gitignore, and fixture/fixture.sub — the same starter project 'subordinate-cli new' writes, produced by the same code path. Effect scaffolds also carry src/effect.wgsl and request `shaders = true`; mcp-tools scaffolds carry schemas/<tool>.json and declare that tool in [mcp.tools]. The generated manifest is parsed with Manifest::parse before it is written, so a scaffold cannot leave a plugin.toml the host would refuse. The JSON report lists every file, the identity, and the three next commands (naming the actual <lib_name>.wasm cargo produces).

CLAUDE.md is generated per world: the exported functions with Rust signatures, the host imports (analyzer also gets analysis-host with its progress/cancellation contract), what the capabilities line means, the fixture and `plugin test <id>` contract with the per-world check TASK-91 will implement (project state for command, a rendered frame for effect, findings for analyzer, a tool call for mcp-tools), the build/install/reload loop, and the house rules (Command API only, RationalTime never floats, a truthful manifest).

AC 2 was proven, not assumed: tests/scaffold.rs::a_scaffold_builds_and_installs_with_no_edits scaffolds all four worlds, runs cargo build --release --target wasm32-wasip2 over each and then installs the resulting component through 'plugin install --dev' and confirms it in 'plugin list'. It is opt-in (SUBORDINATE_SCAFFOLD_BUILD=1 plus SUBORDINATE_SDK_PATH) because a plain 'cargo test' machine may have neither the wasm32-wasip2 target nor an SDK to depend on; it was run here and passed for all four worlds. Two template import paths were corrected because of it: effect's ParamDesc/ParamKind/FloatParam live in bindings::subordinate::plugin::effect_types, not at the bindings root, and Detail comes from the SDK root rather than bindings.

--sdk-path exists because subordinate-sdk is not published yet: without it the scaffold depends on version "0.1", which is what a released SDK will resolve to, and with it on a local checkout, which is what makes 'builds with no edits' true today and in CI.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p subordinate-cli green (38 unit + 5 scaffold integration tests among them), plus the opt-in build/install test above.

Not changed: docs/PLAN.md §6.4 still shows a wasip1 path and 'cargo component build' in its sketch of the loop; the SDK builds components straight from the Rust toolchain for wasm32-wasip2, which is what the scaffold and its CLAUDE.md say. Left alone as the plan of record, worth a correction in a later doc task.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added 'subordinate-cli plugin new --world <world> <name>', which writes a complete plugin crate for the command, effect, analyzer and mcp-tools worlds: cargo project on wasm32-wasip2 with the right SDK feature, a src/lib.rs already implementing that world's Guest trait, a validated plugin.toml, a per-world CLAUDE.md covering the interface, host imports and test contract, and a fixture project made by the same code path as 'subordinate-cli new'. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, cargo test -p subordinate-cli (38 unit tests and 5 integration tests), and an opt-in test that actually builds all four scaffolds for wasm32-wasip2 and installs the resulting components through 'plugin install --dev'.
<!-- SECTION:FINAL_SUMMARY:END -->
