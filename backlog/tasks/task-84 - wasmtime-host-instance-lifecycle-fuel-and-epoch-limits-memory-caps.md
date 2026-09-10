---
id: TASK-84
title: 'wasmtime host: instance lifecycle, fuel and epoch limits, memory caps'
status: Done
assignee:
  - '@opus-task-84'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 04:48'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-83
references:
  - docs/PLAN.md
priority: high
ordinal: 105000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A bad plugin must not stall or crash the engine (§4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Instances are created per plugin with configured memory limit, epoch-based interruption and fuel for CPU-bound calls
- [x] #2 A deliberately infinite loop plugin is terminated and reported without affecting other plugins
- [x] #3 Instance pooling keeps warm instances for hot paths like command and effect describe
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a `runtime` module to sub-plugin: `PluginRuntime` (shared wasmtime Engine with consume_fuel + epoch_interruption + component model, one process-wide epoch ticker thread), `Limits` (fuel, wall-clock deadline, memory/table/instance ceilings), per-plugin `Store` construction with a `StoreLimits` limiter and re-arming of fuel and epoch deadline.
2. Classify abrupt endings into stable SubError codes: plugin.fuel_exhausted, plugin.deadline_exceeded, plugin.trapped, plus load/link/instantiate failures.
3. Add `InstancePool`: warm instances keyed by PluginId with a per-plugin capacity, checkout/release, refuel-on-checkout, eviction for reload, hit/miss counters.
4. Enable the wasmtime cranelift feature so components compile in-process; add a test-only `test-guests` feature whose build script builds wasm32-wasip2 guests (looper, memory hog, stateful counter) with a no_wasm_guests fallback, following the TASK-9 spike.
5. Tests: fuel and epoch both terminate the looper; a terminated plugin leaves other plugins on the same runtime working; the memory cap stops an allocating guest; the pool reuses a warm instance (observable through guest state) and refuels it, and eviction drops it.
6. Verify with cargo fmt, clippy pedantic -D warnings and cargo test -p sub-plugin.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Plan recorded; implementation starting from the TASK-9 spike (spikes/wasm-command-world) which already proved fuel and epoch limits against a wasm32-wasip2 guest.

Implemented crates/sub-plugin/src/runtime.rs: PluginRuntime (shared wasmtime Engine with component model, consume_fuel and epoch_interruption, plus one process-wide ticker thread so N instances cost one thread), Limits (fuel, wall-clock deadline, linear-memory/table/instance ceilings), PluginRuntime::store (installs a StoreLimits resource limiter and arms fuel + epoch deadline) and rearm (resets both, which is what makes a warm instance safe to reuse), Termination/termination() classifying a stopped call by the wasmtime Trap into plugin.fuel_exhausted, plugin.deadline_exceeded or plugin.trapped, and InstancePool (bounded warm instances per PluginId, checkout/release/evict, hit and miss counters). New stable codes in sub_plugin::codes: engine_failed, load_failed, link_failed, instantiate_failed, fuel_exhausted, deadline_exceeded, trapped. wasmtime gained the cranelift feature because the host now compiles components in-process.

A store with no deadline gets u64::MAX/2 ticks, not u64::MAX: wasmtime adds the delta to the engine's current epoch and u64::MAX overflows in a debug build (found by the first run of fuel_stops_a_plugin_that_never_returns).

Three wasm32-wasip2 guests (tests/guests/looper, hog, counter, each its own workspace like the TASK-9 spike guests) are built by a new build.rs, gated on a test-only 'test-guests' feature the crate turns on for itself through a self dev-dependency, so an ordinary build of the library compiles no WASM; when the target is missing the script emits cfg(no_wasm_guests) and tests/runtime_limits.rs compiles out.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-plugin all green (148 tests), including tests/runtime_limits.rs against real components -- fuel and an epoch deadline each stop the looper with the right code, a fuel-terminated looper leaves a counter plugin on the same engine answering (calls 1 then 2) and lets a fresh instance start, a 16 MB ceiling stops the allocating guest as a trap instead of taking the host down, and a pooled counter instance answers 2 on its second checkout with a full fuel budget while eviction resets it to 1.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the wasmtime host to sub-plugin: a shared PluginRuntime (engine, fuel metering, epoch interruption behind one ticker thread), per-instance Limits with a StoreLimits memory/table/instance ceiling, stable-coded terminations, and a bounded InstancePool of warm instances with budgets re-armed on every checkout. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-plugin, including six integration tests that run real wasm32-wasip2 components: fuel and deadline each stop an infinite-loop plugin without disturbing another plugin on the same engine, a memory ceiling stops an unbounded allocator, and a pooled instance is provably reused, refuelled and evictable.
<!-- SECTION:FINAL_SUMMARY:END -->
