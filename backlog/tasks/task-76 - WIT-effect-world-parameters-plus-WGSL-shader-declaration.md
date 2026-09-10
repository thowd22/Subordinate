---
id: TASK-76
title: 'WIT effect world: parameters plus WGSL shader declaration'
status: Done
assignee:
  - '@opus-task-76'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 00:49'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
references:
  - docs/PLAN.md
priority: high
ordinal: 97000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GPU effects are shaders declared by plugins and run by the core (decision-6).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 effect world exports describe() -> EffectDesc { params: list<ParamDesc>, shader: string, entry: string }
- [x] #2 ParamDesc supports float, int, bool, color and enum with ranges and defaults
- [x] #3 Optional process-cpu(frame, params) export for small-buffer processing is defined but marked slow in docs
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add an `effect-types` interface to wit/subordinate-plugin.wit: color, float/int/bool/color/enum param descriptors with ranges and defaults, param-kind variant, param-desc, param-value/param-binding, frame + pixel-format for CPU processing, and effect-desc { params, shader, entry }.
2. Add world `effect`: imports command-api, exports describe() -> effect-desc.
3. Add world `effect-cpu`: includes `effect` and adds the optional export process-cpu(frame, params) -> result<frame, error>, documented as the slow small-buffer path.
4. Generate host bindings for both worlds in crates/sub-plugin/src/bindings.rs (second bindgen reuses the first world's types via `with`), re-export from lib.rs and extend the crate docs.
5. Tests in crates/sub-plugin/tests: build an effect-desc covering every param kind, assert value/kind pairing, and link both worlds into a wasmtime Linker with no unsatisfied imports.
6. Update wit/README.md world table and note the CPU path is slow. Verify with cargo fmt, clippy -D warnings and cargo test -p sub-plugin.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added interface `effect-types` and worlds `effect` and `effect-cpu` to wit/subordinate-plugin.wit (package subordinate:plugin@0.1.0, unchanged version).

Shape: `effect-desc { params: list<param-desc>, shader: string, entry: string }` — WGSL source plus the name of its fragment entry point. `param-desc { id, label, doc, kind }`; `param-kind` is a variant over `float-param { min, max, default, step: option<f32> }`, `int-param { min, max, default }`, `bool-param { default }`, `color-param { default: color }` and `enum-param { variants: list<enum-variant>, default: u32 }`, so every kind carries its range and default. Companion `param-value`/`param-binding` carry runtime values keyed by `param-desc.id`.

Two decisions worth recording:
- The flag case is spelled `boolean`, not `bool`, because `bool` is a WIT keyword and escaping it (`%bool`) would read worse in every generated language.
- Optionality of `process-cpu` is expressed as a second world: `effect` exports only `describe`, and `effect-cpu` is `include effect` plus `process-cpu(frame, params) -> result<frame, error>`. WIT has no optional export, and a plugin that ships only a shader must not be forced to stub one out. Both worlds are documented as slow-path/CPU-fallback in the WIT itself, in wit/README.md and in the sub-plugin crate docs.
- `frame` carries `rational-time`, not seconds: effect timing stays exact like the rest of the timeline.

Host bindings: crates/sub-plugin/src/bindings.rs gained two more `bindgen!` expansions in submodules `effect` and `effect_cpu`, reusing the command world's `types` and `command-api` (and effect's `effect-types`) through `with:`, so there is exactly one Rust `Error`, one `Host` trait and one `EffectDesc` across all three worlds. Effect records derive only `PartialEq` — parameter ranges are f32, so `Eq`/`Hash` are not available.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings exit 0 (with the local GStreamer prefix sourced so sub-media builds); cargo test -p sub-plugin passes 8 unit + 4 effect_world + 4 host_interface + 2 doc tests.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Extended wit/subordinate-plugin.wit (subordinate:plugin@0.1.0) with the effect worlds: interface `effect-types` carries the parameter schema (float, int, bool, colour and enum, each with its range and default) plus `frame`/`param-value` for the CPU path, world `effect` exports `describe() -> effect-desc { params, shader, entry }`, and world `effect-cpu` includes it and adds the optional `process-cpu(frame, params) -> result<frame, error>` export, documented as the slow small-buffer-only path in the WIT, wit/README.md and the sub-plugin crate docs. crates/sub-plugin gained two `bindgen!` expansions reusing the command world's types, so one Rust `Host` trait and one `Error` serve all three worlds. Verified with cargo fmt --all --check, cargo clippy -p sub-plugin --all-targets -D warnings (clean), cargo clippy -p spike-wasm-command-world (clean, so the existing command world still parses and builds against the extended package), and cargo test -p sub-plugin: 18 tests pass, including four new effect_world tests covering every parameter kind, defaults inside declared ranges, value/kind pairing and a frame whose timestamp round-trips exactly at 23.976 fps, plus a linker test that both effect worlds link against the same host and a compile-time check of the generated `describe` and `process-cpu` signatures.
<!-- SECTION:FINAL_SUMMARY:END -->
