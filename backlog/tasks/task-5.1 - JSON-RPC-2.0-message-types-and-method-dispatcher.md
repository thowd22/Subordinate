---
id: TASK-5.1
title: JSON-RPC 2.0 message types and method dispatcher
status: Done
assignee:
  - '@opus-task-5.1'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 13:45'
labels:
  - core
  - mcp
milestone: m-0
dependencies:
  - TASK-12
references:
  - docs/PLAN.md
parent_task_id: TASK-5
priority: high
ordinal: 30000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The Command API is the single surface for GUI, CLI, MCP and plugins (decision-7). Types and dispatch come before transport so they can be unit tested in-process.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Request, Response, Error and Notification types per JSON-RPC 2.0 with batch support
- [x] #2 Dispatcher maps method names (project.open, timeline.add_clip, etc.) to engine commands and queries with typed params
- [x] #3 Unknown method and invalid params return the standard JSON-RPC error codes wrapping SubError
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add rpc module to sub-command: JSON-RPC 2.0 Version marker, RequestId, Request, Notification, Call (untagged), Response with result/error payload, RpcError with the standard code constants, and batch parsing (Message/Outgoing) that rejects empty and malformed batch elements per spec.
2. Add error mapping: RpcError::from_sub_error maps stable SubError codes to JSON-RPC codes (unknown method -> -32601, invalid params -> -32602, internal -> -32603, everything else -> -32000 server error) and carries the full SubError JSON in data.
3. Add dispatch module: Dispatcher over an EngineHandle with a method table. Every registered command kind becomes a mutating method dispatched through EngineHandle::apply_envelope with typed params; query methods (project.get, project.revision, history.get, edit.undo/redo/begin_group/commit_group/abort_group, system.list_methods) decode typed param structs with deny_unknown_fields. Custom methods can be registered so later tasks (project.open, transport) extend the table.
4. Wire sub-command Cargo.toml deps (sub-core, sub-edit, sub-model, serde, serde_json) and crate-level docs with codes module.
5. Tests: unit tests for message parsing/serialisation (batch, notification, id types, bad version), dispatcher tests against a live Engine for command dispatch, queries, unknown method, invalid params, notifications producing no response, and batch responses.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-command: rpc.rs (message layer) and dispatch.rs (method table), wired to sub-core, sub-edit and sub-model.

rpc.rs: Version (a type that only holds "2.0"), RequestId (number or string), Request, Notification, Call (untagged: a call with an id is a Request, one without is a Notification), Response with a flattened result/error Payload, RpcError, and the Incoming/Outgoing batch envelopes. error_codes holds the standard numbers (-32700, -32600, -32601, -32602, -32603) plus -32000 for application failures, and error_codes::for_sub_error maps stable SubError codes onto them: edit.unknown_command/command.unknown_method -> -32601; edit.invalid_command/core.invalid_argument/command.invalid_params -> -32602; core.internal -> -32603; anything else -> -32000. RpcError::from_sub_error keeps the whole SubError JSON in data, so agents still match on the stable code.

dispatch.rs: Dispatcher over an EngineHandle. Every kind on the engine's CommandRegistry becomes a method of the same name (bin.create, track.add, sequence.rename, marker.add, media.import, clip.set_params, ...) whose params are the command's own serde parameters; the dispatcher wraps them in a CommandEnvelope and applies them through EngineHandle::apply_envelope, so a Command API call takes exactly the same undoable path as a UI click (decision-7). Query and history methods are typed with deny_unknown_fields param structs: project.get, project.revision, history.get, edit.undo, edit.redo, edit.begin_group, edit.commit_group, edit.abort_group and system.list_methods. Dispatcher::register adds further methods (plugins, transport) and rejects a duplicate name with the new command.duplicate_method code. Entry points: invoke (name + params), call (Request -> Response), handle_call, handle_incoming, handle_value (spec-correct batch handling: empty batch is an invalid request, a malformed member fails alone with its recovered id) and handle_text (bytes to bytes, including the parse error), which is what the TASK-5.2 transport will call.

New stable codes in sub_command::codes: command.unknown_method, command.invalid_params, command.duplicate_method.

Two notes on scope. (1) sub-edit's register_builtin does not register the clip commands from TASK-4.2 (clip.add, clip.remove, clip.move, clip.trim_in, clip.trim_out, clip.split, clip.ripple_delete), so the engine cannot decode their envelopes and they are therefore not Command API methods yet; the dispatcher derives its command methods from the registry, so they appear automatically once that gap is closed in sub-edit. Left untouched here rather than editing another task's crate. (2) project.open and project.save are not implemented: loading or replacing project state is not an engine operation today, and file I/O is not in this task's criteria. Dispatcher::register is the extension point for both.

Verification: cargo test -p sub-command (37 unit tests + 3 doctests, all pass), cargo fmt --all --check, and cargo clippy --workspace --all-targets -- -D warnings, all clean.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the Command API message layer and dispatcher to crates/sub-command. rpc.rs carries the JSON-RPC 2.0 types (Request, Notification, Call, Response with result/error payload, RpcError, RequestId, Incoming/Outgoing batches) with a Version type that rejects anything but "2.0", plus error_codes mapping stable SubError codes onto the standard numbers and keeping the SubError whole in data. dispatch.rs turns every command kind on the engine's registry into a method applied through EngineHandle::apply_envelope, adds typed query and history methods (project.get, project.revision, history.get, edit.undo/redo, the group methods, system.list_methods) and a register hook for later plugin and transport methods, and answers unknown methods with -32601, bad params with -32602, engine failures with -32000 and malformed messages with -32700/-32600 per the specification, including per-member batch errors. Verified with 37 unit tests and 3 doctests in cargo test -p sub-command, plus cargo fmt --all --check and cargo clippy --workspace --all-targets -- -D warnings clean.
<!-- SECTION:FINAL_SUMMARY:END -->
