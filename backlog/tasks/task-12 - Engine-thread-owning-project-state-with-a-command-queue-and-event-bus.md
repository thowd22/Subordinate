---
id: TASK-12
title: Engine thread owning project state with a command queue and event bus
status: Done
assignee:
  - '@opus-task-12'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 05:21'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-4.4
references:
  - docs/PLAN.md
priority: high
ordinal: 29000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The UI thread must never block and several clients (UI, MCP, plugins) mutate the same project, so a single owner thread serialises commands and broadcasts changes (§4 threading model).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Engine runs on its own thread, accepts commands over a channel and replies with results
- [x] #2 Every applied command emits a ChangeEvent (entity kind, ID, change type) on a broadcast channel
- [x] #3 Read access is via immutable snapshots (Arc) so readers never hold a lock across a frame
- [x] #4 A stress test applies 10k commands from three threads without deadlock or lost events
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add an engine module to sub-edit: EngineThread owning Project + History, driven by an mpsc request queue with per-request reply channels (spawn, shutdown, Drop-joins).
2. Publish read access as immutable Arc<Project> snapshots in a briefly-locked slot plus an atomic revision counter, so readers clone an Arc and never hold a lock.
3. Add ChangeEvent (entity kind, optional id, change type, origin, revision) derived from the command envelope, and a lock-free-enough broadcast bus with bounded per-subscriber queues that drops oldest and counts lag rather than stalling the engine.
4. Emit events for apply, undo and redo (undo inverts added/removed); new edit.* error codes for a stopped engine.
5. Tests: unit tests for event derivation and the bus, integration test crates/sub-edit/tests/engine.rs including a 10k-command three-thread stress test asserting no deadlock, no lost events and monotonic revisions.
6. Verify with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, cargo test -p sub-edit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented the engine in sub-edit rather than a new crate: docs/PLAN.md §4 fixes the crate list, and sub-edit already owns the Project mutation path (Command + History), so crates/sub-edit/src/engine.rs, event.rs and bus.rs make up the engine thread.

Design decisions:
- Engine::spawn/with_config/with_registry start a named 'sub-engine' thread owning Arc<Project>, History and a CommandRegistry. EngineHandle (Clone + Send + Sync) sends Requests over an unbounded mpsc queue and waits on a one-slot reply channel, so submitting never blocks the engine and a dead thread is reported as the new stable code edit.engine_stopped. Engine::shutdown joins; Drop does the same.
- Reads: the engine stores a fresh Arc<Project> in a slot after every successful mutation, and EngineHandle::snapshot clones the Arc out of a momentary lock. Readers hold an immutable snapshot for as long as they like; a revision AtomicU64 (stored after the snapshot) lets a reader see how fresh it is. Mutation is copy-on-write via Arc::make_mut, so an older snapshot is never touched.
- ChangeEvent { revision, entity, id, change, command, origin } is derived from the CommandEnvelope (kind domain -> EntityKind, verb -> ChangeType, params field named after the entity -> id) rather than from a per-command trait method, so plugin command kinds describe themselves without touching the Command trait. Undo emits the inverted change type, redo the original, and an aborted group replays its own events inverted.
- EventBus fans events to weakly-held subscribers with a bounded per-subscriber queue: a slow subscriber loses its oldest events and learns how many from take_lagged(), and the engine is never blocked (docs/PLAN.md §4, TASK-5.3's requirement). Dropping an EventReceiver unsubscribes; stopping the engine closes the bus so blocked recv() calls return None.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (sub-media built with the scratchpad GStreamer sysroot); cargo test -p sub-edit passes 128 tests across lib, four existing integration suites, the new tests/engine.rs and 14 doc tests. The stress test ten_thousand_commands_from_three_threads_lose_no_events applies 10k renames from three threads while a fourth thread reads snapshots, and asserts 10k unique strictly increasing revisions, zero lagged events and a final revision of 10001; it was run five times with no flake.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the engine thread to sub-edit: Engine/EngineHandle (crates/sub-edit/src/engine.rs) own the Project and its History on their own thread, serving commands, undo/redo and grouping over a request queue with per-request reply channels; readers take immutable Arc<Project> snapshots plus a revision counter instead of holding a lock; every applied, undone and redone command broadcasts a ChangeEvent (entity kind, id, change type, origin) derived from the command envelope over a bounded, non-blocking EventBus that reports lag instead of stalling the engine. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, and cargo test -p sub-edit (128 tests), including tests/engine.rs applying 10,000 commands from three threads with a concurrent snapshot reader and asserting unique, strictly increasing revisions and zero lost events.
<!-- SECTION:FINAL_SUMMARY:END -->
