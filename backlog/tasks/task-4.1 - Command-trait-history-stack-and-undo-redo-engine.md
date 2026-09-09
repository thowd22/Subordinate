---
id: TASK-4.1
title: 'Command trait, history stack and undo/redo engine'
status: Done
assignee:
  - '@opus-task-4.1'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 02:15'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-3.6
references:
  - docs/PLAN.md
parent_task_id: TASK-4
priority: high
ordinal: 25000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every mutation must be an undoable command because the same set is exposed to MCP and plugins (decision-7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Command trait with apply(&mut Project) -> Result<Inverse> and a History with undo, redo, clear and a configurable depth
- [x] #2 Commands are serde-serialisable so they can be sent over the Command API and logged
- [x] #3 Grouping API lets several commands undo as one step (needed for drags)
- [x] #4 Tests prove undo then redo yields JSON-identical project state
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-edit gains a Command trait: concrete, serde-serialisable command types implement apply(&self, &mut Project) -> SubResult<Inverse>, where Inverse wraps another boxed command.
2. Object-safe AnyCommand blanket impl plus BoxedCommand, a stable CommandEnvelope {kind, params} JSON shape and a CommandRegistry that decodes envelopes back into commands for the Command API and logs.
3. History: undo/redo stacks of entries, each entry a list of (forward, inverse) steps; configurable depth with oldest-entry trimming, clear, labels, entry inspection.
4. Grouping API: begin_group/commit_group/abort_group so a drag applies many commands that undo as one step, with automatic rollback if a command inside a group fails.
5. sub-edit error codes (edit.*) for unknown/invalid/duplicate commands and misuse of the grouping API.
6. Tests: unit tests for registry, depth, grouping, rollback; integration tests proving undo-then-redo round-trips to JSON-identical project text via sub_model::json.
7. Verify with cargo fmt --check, clippy pedantic -D warnings, cargo test -p sub-edit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-edit:

- command.rs: Command trait (const KIND, apply(&self, &mut Project) -> SubResult<Inverse>, label), object-safe AnyCommand blanket impl, BoxedCommand, Inverse (the undo is itself a command, so redo is the inverse of the inverse), CommandEnvelope {kind, params} and CommandRegistry (register/decode/decode_all/kinds).
- history.rs: History with undo/redo stacks of entries; each entry holds (forward, inverse) steps that are swapped as it is replayed, so repeated undo/redo cycles stay exact. Configurable depth (DEFAULT_DEPTH = 100, with_depth/set_depth reject 0 with core.invalid_argument and trim the oldest steps), clear, labels, undo_entries/redo_entries and to_envelopes for the future history panel and log.
- Grouping: begin_group/commit_group/abort_group. Commands applied inside a group become one undo step; a failure inside a group rolls back the commands it already applied; groups do not nest and undo/redo are refused while one is open (edit.group_open).
- codes: edit.unknown_command, edit.invalid_command, edit.duplicate_command, edit.group_open, edit.no_group.

Design notes: commands are concrete serde types rather than a single enum, so TASK-4.2..4.4 (and plugins) can add kinds without touching this crate; the registry is what turns Command API / MCP JSON back into an applicable command. Commands must be atomic (validate then mutate) because nothing rolls back a half-applied single command; the history documents that and rolls back groups.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the scratchpad GStreamer env sourced so sub-media builds); cargo test -p sub-edit = 17 unit + 6 integration + 4 doc tests, all passing.

Evidence per acceptance criterion:
- AC1: crates/sub-edit/src/command.rs and history.rs; history unit tests undo_and_redo_walk_the_stack, depth_drops_the_oldest_steps, clear_forgets_both_stacks.
- AC2: a_command_round_trips_through_its_envelope asserts the exact envelope JSON; integration test commands_decoded_from_json_apply_and_undo_the_same_way sends a command through serde_json and the registry and applies it.
- AC3: a_group_undoes_as_one_step, aborting_a_group_rolls_the_project_back, a_group_of_commands_undoes_and_redoes_as_one_step (four commands, one undo step).
- AC4: crates/sub-edit/tests/undo_redo.rs compares sub_model::json::to_json text before/after across two full undo-redo cycles (undo_then_redo_restores_json_identical_state), step by step (a_partial_undo_matches_the_state_that_step_produced), for a group, and for a depth-bounded history.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Built the command engine in sub-edit: a serde-based Command trait whose apply returns the Inverse that undoes it, a type-erased AnyCommand plus CommandEnvelope/CommandRegistry so commands travel and are logged as stable {kind, params} JSON, and a History with configurable depth, clear, labels and a begin/commit/abort grouping API that makes a drag one undo step and rolls back a group whose command fails. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test -p sub-edit (27 tests), including tests/undo_redo.rs which asserts the sub_model::json project text is identical before and after full undo-then-redo cycles.
<!-- SECTION:FINAL_SUMMARY:END -->
