---
id: TASK-9
title: >-
  Write WIT for the command world and run a hello-world component through
  wasmtime
status: Done
assignee:
  - '@opus-task-9'
created_date: '2026-09-08 20:53'
updated_date: '2026-09-09 20:20'
labels:
  - spike
  - plugins
milestone: m-6
dependencies:
  - TASK-5
references:
  - docs/PLAN.md
priority: high
ordinal: 9000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Plugins are WASM components with versioned WIT worlds following Zed's pattern (decision-6). The command world is the first and simplest: a plugin that calls back into the Command API host import. This proves the host-import shape before the other worlds are designed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 wit/subordinate-plugin.wit defines subordinate:plugin@0.1.0 with a command world and a command-api host interface
- [x] #2 A Rust component built with cargo component adds a marker to a fixture project through the host import
- [x] #3 wasmtime fuel or epoch limits terminate a deliberately looping plugin
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Write wit/subordinate-plugin.wit: package subordinate:plugin@0.1.0 with a command-api host interface (import) and a command world exporting the plugin entry point.
2. Add spikes/wasm-command-world: a wasmtime host that implements command-api against a tiny in-memory fixture project, plus two guest crates (marker, looper) built for wasm32-wasip2 with wit-bindgen (no cargo-component needed since Rust 1.82 emits components for wasip2).
3. build.rs builds the guests into OUT_DIR with a separate target dir; when the wasm32-wasip2 target is missing it emits a cfg so the wasm tests compile out rather than fail.
4. Tests: (a) the marker guest adds a marker to the fixture project through the host import; (b) the looper guest is terminated by wasmtime fuel exhaustion and by an epoch deadline.
5. Record findings in a backlog doc (spike deliverable, per CLAUDE.md), covering the WIT shape, the toolchain route, fuel vs epoch, and what the real sub-plugin host should adopt.
6. Verify with cargo fmt --check, clippy -D warnings and cargo test.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Deliverable is wit/subordinate-plugin.wit plus backlog doc-2 (spike findings); spikes/wasm-command-world is the throwaway proof.

AC1: wit/subordinate-plugin.wit declares subordinate:plugin@0.1.0 with interface types (error, rational-time), interface command-api (invoke, query, playhead, log) and world command importing command-api and exporting run. Verified by both wasmtime bindgen! (host) and wit_bindgen::generate! (guests) parsing it and the resulting components linking.

AC2 with one toolchain deviation: the Rust component was NOT built with cargo component, which is no longer needed. Since Rust 1.82 the wasm32-wasip2 target emits a component directly, so the guest is a plain cdylib crate using wit-bindgen 0.61 and cargo build --target wasm32-wasip2. The substance of the criterion is proven: test marker_plugin_edits_the_project_through_the_host_import loads the component, and the plugin reads the playhead and calls sequence.add_marker through the command-api host import; the fixture project ends with exactly one marker at the exact playhead RationalTime and one applied command. marker_plugin_error_crosses_back_as_a_sub_error proves the error path.

AC3: fuel_exhaustion_terminates_a_looping_plugin (1 000 000 fuel, stops having burned exactly 1 000 000, Trap::OutOfFuel) and an_epoch_deadline_terminates_a_looping_plugin (200 ms deadline, stopped at 209.7 ms, Trap::Interrupt).

Design changes made during the spike, recorded in doc-2: rational-time carries the rate as numerator/denominator rather than a single u32 ticks-per-second, because 23.976 and 29.97 fps are not integral; invoke/query pass JSON params so the Command API schema stays the single source of truth; StoreLimits instance ceiling must be well above 1 because one component is several core instances.

CI: added targets: wasm32-wasip2 to the toolchain step in .github/workflows/ci.yml, otherwise the four WASM tests compile out silently via cfg(no_wasm_guests).

Verification run here: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the local GStreamer prefix sourced); cargo test -p spike-wasm-command-world 7 passed, 0 failed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Wrote wit/subordinate-plugin.wit (subordinate:plugin@0.1.0: types, command-api host import, command world) and proved it end to end in spikes/wasm-command-world: a wasmtime 48 host implementing command-api over a fixture project, plus two wasm32-wasip2 guest components built by build.rs. The marker guest reads the playhead and adds a marker through the host import, and the looper guest is terminated both by fuel exhaustion and by an epoch deadline. Findings, measurements and the recommendations for TASK-75 to TASK-84 are in backlog doc-2. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, and cargo test -p spike-wasm-command-world (7 passed).
<!-- SECTION:FINAL_SUMMARY:END -->
