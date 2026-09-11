---
id: TASK-128
title: 'plugin new: make the scaffolded crate build on a default toolchain'
status: Done
assignee:
  - '@opus-task-128'
created_date: '2026-09-10 21:58'
updated_date: '2026-09-11 00:36'
labels:
  - plugins
  - cli
milestone: m-6
dependencies:
  - TASK-102
priority: medium
ordinal: 148000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The TASK-102 runbook run hit this: the scaffolded crate declares rust-version = 1.95 (the SDK's floor) but writes no rust-toolchain.toml, so on a machine whose default toolchain is older, 'cargo build --release --target wasm32-wasip2' fails with a rust-version error the agent has to diagnose and work around (it found 'rustup run 1.95.0'). The scaffold's promise is that what it writes builds with no edits, so either it pins the toolchain it needs or its CLAUDE.md says in the build step which toolchain to use and how.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A crate written by plugin new builds with the plain command its CLAUDE.md prints, on a machine whose default toolchain is older than the SDK's rust-version
- [x] #2 The scaffold test covering the build (SUBORDINATE_SCAFFOLD_BUILD=1) exercises that path
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a TOOLCHAIN_CHANNEL constant to bins/subordinate-cli/src/scaffold.rs and derive the Cargo.toml rust-version from it.
2. Write a rust-toolchain.toml into every scaffolded crate pinning that channel and targets = ["wasm32-wasip2"], so rustup selects and installs what the crate needs regardless of the machine's default toolchain.
3. Say so in the generated CLAUDE.md build loop: run the command from the crate directory, the pin handles the toolchain and the target.
4. Unit tests: the file is written, reported, its channel agrees with rust-version, and it names the target.
5. Build test (SUBORDINATE_SCAFFOLD_BUILD=1): run the plain 'cargo build --release --target wasm32-wasip2' through the rustup shim from the crate directory with RUSTUP_TOOLCHAIN/CARGO/RUSTC removed, so the scaffold's own pin is what picks the toolchain, and assert rustup reports the pinned channel active there.
6. fmt, clippy pedantic, cargo test -p subordinate-cli, then finalize.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
plugin new now writes a rust-toolchain.toml into every scaffolded crate: channel = "1.95.0" (the TOOLCHAIN_CHANNEL constant, from which the crate's rust-version = "1.95" is derived) and targets = ["wasm32-wasip2"], so rustup selects and, if needed, installs both from the crate directory. The generated CLAUDE.md now says the pin is why no toolchain flag is needed and that the build is run from the crate root.

Evidence on this machine, whose default toolchain is 1.93.1 (rustup default = stable = rustc 1.93.1), older than the SDK's floor:
- In a scaffolded crate: 'rustup show active-toolchain' (with RUSTUP_TOOLCHAIN/CARGO/RUSTC unset) reports '1.95.0-x86_64-unknown-linux-gnu (overridden by .../built-command/rust-toolchain.toml)'.
- tests/scaffold.rs::a_scaffold_builds_and_installs_with_no_edits now runs the plain 'cargo build --release --target wasm32-wasip2' through the rustup shim with current_dir set to the crate and every inherited toolchain override stripped, and asserts rustup resolves the pinned channel there before building. SUBORDINATE_SCAFFOLD_BUILD=1 SUBORDINATE_SDK_PATH=<repo>/sdk/subordinate-sdk cargo test -p subordinate-cli: 5 scaffold tests pass (all four worlds built and installed, 53.87s).
- New unit test scaffold.rs::the_scaffold_pins_the_toolchain_its_rust_version_needs checks the file's channel and target, that the channel is not below the declared rust-version, and that the guide mentions both.

Checks: cargo fmt --all --check clean; cargo clippy -p subordinate-cli --all-targets -- -D warnings clean; cargo test -p subordinate-cli all green (with the GStreamer env sourced). Workspace-wide clippy was not run here because other agents held the shared target directory; the touched crate was linted.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Scaffolded plugin crates now carry a rust-toolchain.toml pinning Rust 1.95.0 and the wasm32-wasip2 target, so the single build command the generated CLAUDE.md prints works unchanged on a machine whose default toolchain is older than the SDK's rust-version (this machine defaults to 1.93.1); the guide explains the pin and that the build runs from the crate root. Verified by the scaffold build test, which now runs the plain cargo command through the rustup shim from the crate directory with inherited toolchain overrides stripped and asserts rustup resolves the pinned channel, plus a unit test over the generated file; fmt, clippy and cargo test -p subordinate-cli all pass.
<!-- SECTION:FINAL_SUMMARY:END -->
