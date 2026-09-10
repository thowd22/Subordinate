---
id: TASK-78
title: WIT importer and exporter worlds
status: In Progress
assignee:
  - '@opus-task-78'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 00:51'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
references:
  - docs/PLAN.md
priority: medium
ordinal: 99000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Interchange and custom export targets live in plugins (§6.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 importer world: supported-extensions() and import(path) -> list<MediaOrSequenceSpec> applied by the host via commands
- [x] #2 exporter world: presets() -> list<PresetDesc> and optional post-export(path) hook
- [ ] #3 Host wires importer results into the bin and exporter presets into the export panel
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Extend wit/subordinate-plugin.wit with an `importer` interface (supported-extensions, import(path) -> list<media-or-sequence-spec> with media/clip/gap/transition/track/sequence/marker specs) and an `exporter` interface (presets() -> list<preset-desc>, post-export(path) hook), plus `world importer` and `world exporter` that import command-api.
2. Generate host bindings for both worlds in sub-plugin (bindgen! submodules reusing the existing types/command-api bindings via `with`).
3. Host wiring: new sub-plugin module turning importer specs into Command API calls (media.import + sequence.insert JSON) applied through the host's command queue, and exporter preset records into a host-side preset list the export panel (TASK-62) consumes.
4. Tests: WIT worlds link into a wasmtime Linker with no unsatisfied imports; import plan applies against a real sub-edit engine/dispatcher so media lands in the bin and the sequence is built; preset conversion round-trips and rejects malformed input.
5. Verify with cargo fmt/clippy pedantic/test; finalize the task.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
WIT: wit/subordinate-plugin.wit gains interface importer-api (supported-extensions, %import(path) -> list<media-or-sequence-spec>, with media/clip/gap/crossfade/track/sequence/marker specs and a media-ref variant resolving either an item this same import produced or one already in the project) and interface exporter-api (presets() -> list<preset-desc>, post-export(path)), plus worlds importer and exporter that import command-api. Interfaces are named *-api because a WIT package shares one namespace between worlds and interfaces, matching the existing command-api/command pair; 'import' is a WIT keyword so the function is written %import and is still named import in the component model. post-export is declared unconditionally: WIT has no optional export, so the contract is that a preset-only exporter returns ok and the SDK ships that as its default.

Host side: sub-plugin gains bindings::importer and bindings::exporter (bindgen with: maps types and command-api onto the existing generated ones, so one Host impl links into all three worlds) and a new interchange module. import_plan() turns an importer result into Command API calls -- one media.import per file, emitted before any sequence whatever order the plugin listed things in, then one sequence.insert per sequence carrying the whole built sequence -- so an import is a handful of ordinary undoable commands and every id is minted host-side. export_presets() validates preset-desc records into ExportPreset rows (exact Rational frame rate, open JSON settings) for the export panel. New codes: plugin.invalid_spec, plugin.invalid_preset.

AC #3 is half done and left unchecked: the importer half is proven end to end (tests/interchange.rs applies a plan through the real Dispatcher over a real Engine; the media lands in the root bin, the sequence arrives at 24000/1001 with its gap and clips, and one edit.undo per call backs the whole import out). The export panel does not exist yet -- TASK-62 is still To Do -- so the exporter half stops at validated ExportPreset rows with no panel to list them in.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (5m42s, GStreamer env sourced); cargo test -p sub-plugin = 18 unit + 4 host_interface (both new worlds link into a wasmtime Linker with no unsatisfied imports) + 3 interchange + 2 doc tests, all passing.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the importer and exporter WIT worlds and their host wiring. wit/subordinate-plugin.wit declares importer-api (supported-extensions, %import(path) -> list<media-or-sequence-spec>) and exporter-api (presets, post-export), each in a world importing command-api; sub-plugin generates host bindings for both and its new interchange module turns an importer result into media.import + sequence.insert Command API calls and validates preset records into ExportPreset rows. Verified with cargo fmt --check, workspace clippy -D warnings, and 27 passing sub-plugin tests, including an end-to-end import applied through the real JSON-RPC dispatcher over a live engine and undone again. AC #3 is left unchecked: its importer half is proven, but the export panel it names does not exist yet (TASK-62 is To Do).
<!-- SECTION:FINAL_SUMMARY:END -->
