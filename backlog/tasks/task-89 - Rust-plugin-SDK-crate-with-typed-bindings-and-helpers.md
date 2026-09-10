---
id: TASK-89
title: Rust plugin SDK crate with typed bindings and helpers
status: Done
assignee:
  - '@opus-task-89'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 10:12'
labels:
  - plugins
  - sdk
milestone: m-6
dependencies:
  - TASK-80
  - TASK-76
  - TASK-81
references:
  - docs/PLAN.md
priority: high
ordinal: 110000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Agents write most plugins in Rust; the SDK hides WIT boilerplate.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 subordinate-sdk crate wraps guest bindings with ergonomic types, a run_command helper and serde-based param structs
- [x] #2 Docs.rs-style documentation with an example per world
- [x] #3 Published under MIT OR Apache-2.0 (decision-2)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Feature-gate one wit-bindgen world per cargo feature (command default, plus effect, effect-cpu, commands, mcp-tools, audio-effect, importer, exporter, analyzer); exactly one may be on, checked by a const assertion. Every world imports command-api and types, so the helper layer compiles unchanged whichever world is selected.
2. Implement serde Serialize/Deserialize by hand for the generated WIT value records (the six id records as plain strings, rational, rational-time, time-range, track-kind), so param structs can use the WIT types directly and produce exactly the Command API's wire JSON.
3. Add ergonomic constructors and exact comparison helpers for Rational/RationalTime/TimeRange (i128 cross-multiplication, no floats) and an Id trait over the six identifier records.
4. Add a params module: a CommandParams trait carrying METHOD and Output, with serde structs for the clip, track, sequence and marker commands whose parameters are ids, times and primitives, plus typed query params/results (project.revision, history.get, edit.undo/redo, group methods).
5. Add a host module: run_command/query typed helpers over command-api, run_command_json/query_json escape hatches for model-carrying commands, a Project handle wrapping the metadata accessors, log helpers and SubError-shaped Error constructors.
6. Docs: crate-level documentation with a worked example per world, and per-item docs throughout.
7. Tests: round-trip tests for the serde impls, wire-shape tests that deserialise each SDK param struct as the matching sub-edit command struct (host-only dev-dependency, so the wasm build is unaffected), time comparison tests.
8. Verify cargo fmt, cargo clippy --workspace --all-targets -D warnings, cargo test -p subordinate-sdk, and a build of every non-default world feature.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

- `src/bindings.rs` now generates one WIT world per cargo feature (`command` default, plus effect, effect-cpu, commands, mcp-tools, audio-effect, importer, exporter, analyzer). A const assertion turns zero or more than one selected world into a compile error naming the features, because a component implements exactly one world and two generated world descriptions in one module would leave the componentiser demanding the exports of both. Every world imports `command-api`, which uses `types`, so the helper layer below compiles identically in all nine.
- `src/wire.rs` hand-writes serde for the generated value records: identifiers as plain JSON strings (not `{"value": …}`), rational as its two unreduced integers, rational-time as `{value, rate}`, time-range as `{start, duration}`, track-kind as the engine's snake_case word, and error as `{code, message, details{}}`. Nothing rounds and nothing goes through f64.
- `src/time.rs` gives exact timeline arithmetic: named broadcast rates as exact fractions, frames/seconds/zero/range constructors, and compare/add/sub/rescale/contains that cross-multiply in i128. An inexact rescale or an overflow returns None rather than rounding.
- `src/ids.rs` adds the `Id` trait (parse/as_str/into_string) over all six identifier records without making them interchangeable.
- `src/params.rs` adds the `CommandParams` trait (METHOD + Output) and serde structs for the sixteen clip, track, sequence and marker commands whose parameters are identifiers, times and primitives, plus typed query params and results for project.get, project.revision, history.get, edit.undo/redo and the group methods. Commands carrying a whole project entity (clip.add, track.insert, sequence.create, marker.add) are deliberately absent: mirroring the project model in the SDK would be a second definition that drifts, and Project::run_json covers them.
- `src/host.rs` adds the run_command helper layer: `Project::run`/`query` take a typed parameter struct and return its typed result, `run_json`/`query_json` are the untyped escape hatches, the metadata accessors and `clips_in` wrap the read-only imports, plus log helpers and SubError-shaped error constructors. A serde failure inside the SDK becomes plugin.invalid_argument or plugin.invalid_result carrying the method name and the serde message.
- `src/lib.rs` carries the crate documentation, including one worked example per world, and re-exports the surface. `Guest` is re-exported except under importer/exporter, whose worlds export an interface rather than bare functions.
- CI gains a Linux-only step linting the eight non-default world features, so a WIT change that breaks a world is caught in CI rather than by the first plugin to target it.

Verification

- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean (with the local GStreamer env sourced for sub-media).
- cargo clippy -p subordinate-sdk --no-default-features --features <world> -- -D warnings: clean for all eight non-default worlds.
- cargo test -p subordinate-sdk: 29 unit tests and 5 doctests pass. The parameter-struct tests serialise each SDK struct and deserialise the JSON as the engine's own sub_edit command struct, which is deny_unknown_fields, so a renamed or missing field fails there; a further test asserts every METHOD is a kind sub_edit::builtin_registry registers. sub-edit/sub-model/sub-time are dev-dependencies only, so a plugin building for wasm never sees them.
- cargo build -p subordinate-sdk --target wasm32-wasip2: succeeds, which is the target a plugin actually builds for.
- RUSTDOCFLAGS="-D warnings" cargo doc -p subordinate-sdk --no-deps: clean, no broken intra-doc links.

AC #3 note: the crate declares license = "MIT OR Apache-2.0" and sdk/LICENSE-MIT and sdk/LICENSE-APACHE carry the texts, per decision-2. Nothing in this repository publishes to crates.io yet, so the criterion is met as the declared licence rather than as an actual registry release.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Turned subordinate-sdk from bare generated bindings into the plugin SDK proper. It now generates one WIT world per cargo feature with a compile-time guard that exactly one is selected, hand-writes serde for the generated value records so the WIT types are the parameter types, adds exact float-free timeline arithmetic and an Id trait, adds a CommandParams trait with typed parameter structs for the clip, track, sequence and marker commands and the built-in queries, and adds a Project handle whose run/query helpers carry a typed struct to the host and a typed result back, with run_json/query_json as the escape hatch for the model-carrying commands. Crate documentation includes a worked example for each of the nine worlds. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings plus per-world clippy for the eight non-default features, cargo test -p subordinate-sdk (29 unit tests and 5 doctests, including tests that deserialise every SDK parameter struct as the engine's own deny_unknown_fields command struct), a wasm32-wasip2 build, and a warnings-as-errors cargo doc.
<!-- SECTION:FINAL_SUMMARY:END -->
