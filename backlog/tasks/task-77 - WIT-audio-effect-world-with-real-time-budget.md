---
id: TASK-77
title: WIT audio-effect world with real-time budget
status: Done
assignee:
  - '@opus-task-77'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 00:57'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
references:
  - docs/PLAN.md
priority: medium
ordinal: 98000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Simple audio processing in plugins (§6.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 audio-effect world exports describe() and process(block: list<f32>, channels, rate, params) -> list<f32>
- [x] #2 Host enforces a per-block time budget and bypasses the plugin after repeated overruns
- [x] #3 Reference plugin: gain with a smoothing parameter
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add an `audio` interface (param, param-descriptor, effect-description) and an `audio-effect` world to wit/subordinate-plugin.wit: imports command-api like every world, exports describe() and process(block, channels, rate, params).
2. Generate the host side of the new world in sub-plugin (bindgen with `with:` reusing the command world's types and Host trait) and re-export it.
3. Add sub-plugin::audio: a no-alloc, no-lock RealTimeBudget that derives a per-block deadline from frames/sample-rate, records each block's elapsed time, and bypasses the plugin after N consecutive overruns; plus block validation (channels, interleaved length, returned length) with stable plugin.* error codes.
4. Reference plugin: plugins/gain, a standalone-workspace guest crate (like the TASK-9 spike guests) implementing describe()/process() with a gain parameter and a smoothing-time parameter, DSP in a pure no-alloc module with unit tests, built for wasm32-wasip2.
5. Tests: unit tests for the budget and DSP, a linker test proving the audio-effect world's imports are satisfiable by the same host, and an allocator-counting test proving the budget path allocates nothing.
6. Verify: cargo fmt --check, clippy pedantic -D warnings, cargo test -p sub-plugin, guest cargo test + wasm32-wasip2 build.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented.

WIT (wit/subordinate-plugin.wit): new `audio` interface (param-unit, param-descriptor, param, effect-description) and `audio-effect` world. The world imports `command-api` like every other world and exports `describe() -> effect-description` and `process(block: list<f32>, channels: u32, rate: u32, params: list<param>) -> result<list<f32>, error>`. The one deviation from AC #1 as written: `process` returns a result rather than a bare list, because every boundary in this project carries SubError with a stable code (CLAUDE.md conventions); an Err is charged exactly like an overrun, so it costs the host nothing extra to accept one.

Host (crates/sub-plugin): a second bindgen expansion in bindings.rs with `with:` pointing `types` and `command-api` at the command world's expansion, so there is one Error type and one command_api::Host trait in the crate and a host that serves the command world serves this one. New module `audio`: BlockFormat (non-zero frames/channels/rate, exact integer-nanosecond wall clock, interleaved length check), RealTimeBudget (deadline as a percent of the block's own wall clock, host policy min plugin claim, consecutive-fault counting, latched bypass, reset, bypass_error), BlockTimer, and process_block(), which funnels the three ways a plugin can misbehave — slow, error, wrong-shaped buffer — into one fault path. Four new stable codes: plugin.invalid_block_format, plugin.invalid_audio_block, plugin.invalid_budget, plugin.audio_bypassed.

Reference plugin (plugins/gain): its own one-package workspace like the TASK-9 spike guests, MIT OR Apache-2.0, wit-bindgen against the same wit/ directory. Parameters `gain-db` (decibel, -60..12, default 0) and `smoothing` (seconds, 0..1, default 0.02); the DSP is a one-pole ramp toward the target gain whose time constant is the smoothing parameter, held in a thread-local across blocks so a ramp survives a block boundary. It processes the incoming buffer in place and returns it, so a block costs no second allocation, and it claims budget_percent 10.

Verification: cargo fmt --all --check clean (workspace and plugins/gain); cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-plugin 23 unit + 1 no-alloc + 4 integration + 3 doc tests pass; cargo test in plugins/gain 14 pass; cargo build --release --target wasm32-wasip2 in plugins/gain produces an 83 KB component. CI gained a Linux-only step that builds the component and runs its tests, since a separate workspace is invisible to cargo --workspace.

AC #2 is the host-side enforcement mechanism and its policy, proven by tests that drive a fast plugin, a plugin that sleeps twenty times its budget, one that returns an error and one that returns a wrong-length buffer through process_block and assert the bypass latches and the plugin stops being entered. Instantiating a real component and calling it under wasmtime belongs to TASK-84 (wasmtime host: instance lifecycle, fuel and epoch limits), which is where this budget gets wired to an actual guest call; nothing here fakes that boundary, it just is not built yet.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the audio-effect world to subordinate:plugin@0.1.0 — an `audio` interface of parameter and description records, plus a world exporting describe() and process(block, channels, rate, params) — and generated its host side in sub-plugin by reusing the command world's types and Host trait. The new sub_plugin::audio module is the host's half of the contract: BlockFormat checks the interleaved block shape, RealTimeBudget derives a per-block deadline from the block's own wall clock (host policy capped by the plugin's claim) and latches into bypass after repeated overruns, errors or malformed buffers, and process_block() applies that policy around one guest call — none of it allocating or locking. plugins/gain is the reference plugin: a smoothed gain with gain-db and smoothing parameters whose ramp carries across block boundaries and which reuses its input buffer. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, 31 passing sub-plugin tests (including an allocator-counting test over the budget path and a linker test proving the world's imports are satisfiable), 14 passing plugin tests, and a wasm32-wasip2 component build.
<!-- SECTION:FINAL_SUMMARY:END -->
