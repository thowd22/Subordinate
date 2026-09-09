---
id: TASK-75
title: 'WIT host interface: command-api, logging and project queries'
status: Done
assignee:
  - '@opus-task-75'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 23:42'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-9
  - TASK-5.4
references:
  - docs/PLAN.md
priority: high
ordinal: 96000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every plugin world imports the same host interface; it is the plugin-side view of the Command API (decision-6, decision-7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 wit/ defines subordinate:plugin@0.1.0 with a host interface exposing run-command(json) -> result, query(json) -> json, log(level, msg) and project metadata accessors
- [x] #2 Types use WIT records for RationalTime and IDs rather than opaque strings where practical
- [x] #3 wit-bindgen generates host bindings in sub-plugin and guest bindings in the SDK crate without warnings
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Extend wit/subordinate-plugin.wit (subordinate:plugin@0.1.0): add id records (project-id, sequence-id, track-id, clip-id, marker-id, media-id), a rational record, and time-range; rename command-api.invoke to run-command so the host interface reads run-command(json) -> result; keep query and log; add project metadata accessors (project-info, sequences, tracks, playhead) returning records built from those id and rational-time types. Document every item so wit-bindgen emits docs.
2. sub-plugin: add wasmtime and generate host bindings with wasmtime::component::bindgen! into a narrowly-allowed private module; add lossless conversions between the WIT records and sub_core::SubError, sub_time::RationalTime/Rational and sub_model ids, with round-trip tests, plus a test Host impl proving the generated trait shape.
3. subordinate-sdk: add wit-bindgen and generate guest bindings into a bindings module with the same narrow allow list; re-export the documented types from the crate root. Typed helpers beyond this belong to TASK-89.
4. Update spikes/wasm-command-world (host impl and both guests) for the renamed function and the new id records so the WIT keeps its end-to-end proof.
5. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test -p sub-plugin -p subordinate-sdk -p spike-wasm-command-world.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
AC1: wit/subordinate-plugin.wit still declares subordinate:plugin@0.1.0 with the one command-api host interface, now carrying the full plugin-side view of the Command API: run-command(project, method, params) -> result<string, error> (renamed from the spike's invoke so the JSON command entry point is named for what it is), query(...) -> result<string, error>, log(level, message), and the metadata accessors open-projects, project-info, sequences, tracks, clips, markers and playhead. The accessors return records rather than JSON because their shape is fixed; run-command and query stay JSON so the Command API schema (TASK-5.4) remains the single source of truth and a new method needs no WIT revision. Proven by both generators parsing the file and by the spike's end-to-end test still adding a marker through the import.

AC2: interface types now holds rational, rational-time (rate carried as a nested rational record, not a flat pair), time-range, and one record per identifier: project-id, sequence-id, track-id, clip-id, marker-id, media-id. Distinct records mean a clip-id cannot be passed where a track-id is wanted, on either side of the boundary. The world export is run(project: project-id, args: string). What deliberately stays a string is the JSON payload of run-command and query, and the UUID text inside each id record, which is the wire form sub-model itself uses.

AC3: host bindings live in crates/sub-plugin/src/bindings.rs via wasmtime component bindgen and guest bindings in sdk/subordinate-sdk/src/bindings.rs via wit_bindgen generate (pub_export_macro plus default_bindings_module, so a plugin crate writes subordinate_sdk::bindings::export!(Plugin)). Both compile with no warnings under cargo clippy --workspace --all-targets -- -D warnings, missing_docs included, and subordinate-sdk also builds and clippies clean for wasm32-wasip2. The generated code itself is exempted from missing_docs, clippy::all and clippy::pedantic inside those two modules only, since the expansion is not hand-written; every hand-written item stays under the workspace lints.

crates/sub-plugin/src/convert.rs is the one place the two vocabularies meet: SubError to and from the error record (cause deliberately dropped, an unparsable plugin code becomes plugin.invalid_error_code with the original kept as a detail), RationalTime/Rational/TimeRange to and from their records (fallible inbound: plugin.invalid_rational, plugin.invalid_time_range), every sub-model id to and from its record, plus the project_metadata, sequence_metadata, track_metadata, track_clip_metadata and marker_metadata builders. tests/host_interface.rs implements the generated Host trait over a real sub-model project, checks every accessor, and links the world into a wasmtime Linker with no import left unsatisfied.

The spike was updated, not left behind: spikes/wasm-command-world (host plus both wasm32-wasip2 guests) tracks the rename, the nested rate and the new accessors, so wit/subordinate-plugin.wit keeps its end-to-end proof. The fixture project has no sequences or tracks, so its sequences/tracks/clips/markers return plugin.unsupported; the real answers live in sub-plugin's builders and are covered by sub-plugin's tests. Backlog doc-2 (TASK-9 findings) still describes the older invoke name and the flat rate fields; it is a dated findings record, not the interface of record.

Verification run here: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the local GStreamer prefix sourced); cargo clippy -p subordinate-sdk --target wasm32-wasip2 -- -D warnings clean; cargo test -p sub-plugin -p subordinate-sdk -p spike-wasm-command-world all pass (8 unit plus 3 integration plus 2 doctests for sub-plugin, 2 unit plus 1 doctest for the SDK, and the spike's 7, which rebuild both guest components against the new WIT).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Grew wit/subordinate-plugin.wit into the shared host interface every plugin world will import: command-api now exposes run-command and query (JSON in, JSON out, so the Command API schema stays the source of truth), log, and the record-returning accessors open-projects, project-info, sequences, tracks, clips, markers and playhead, over a types interface of rational, rational-time, time-range and a distinct record per identifier. Generated the host half in sub-plugin (wasmtime bindgen plus a conversion layer onto SubError, RationalTime and the sub-model ids, with metadata builders) and the guest half in subordinate-sdk (wit-bindgen, exportable from a plugin crate), and updated the TASK-9 spike so the WIT keeps its end-to-end proof. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo clippy -p subordinate-sdk --target wasm32-wasip2 -- -D warnings, and cargo test -p sub-plugin -p subordinate-sdk -p spike-wasm-command-world (all pass).
<!-- SECTION:FINAL_SUMMARY:END -->
