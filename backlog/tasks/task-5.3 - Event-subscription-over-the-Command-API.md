---
id: TASK-5.3
title: Event subscription over the Command API
status: Done
assignee:
  - '@opus-task-5.3'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 15:48'
labels:
  - core
  - mcp
milestone: m-0
dependencies:
  - TASK-5.2
references:
  - docs/PLAN.md
parent_task_id: TASK-5
priority: medium
ordinal: 32000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
MCP clients and pop-out windows need to know when the project changes without polling.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 events.subscribe returns a subscription ID and streams ChangeEvent notifications; events.unsubscribe stops them
- [x] #2 Slow subscribers are dropped after a bounded backlog with a logged warning rather than stalling the engine
- [x] #3 Test proves a second client sees a change made by the first
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New module sub-command/src/events.rs: per-connection Session holding an Outbox (bounded queue of outbound lines) and a map of subscriptions; each subscription owns an EventReceiver from EngineHandle::subscribe plus a pump thread that turns ChangeEvents into events.changed notifications.
2. Slow-subscriber policy: the outbox is bounded; when it is full the session is closed with a logged warning and the connection is dropped, so the engine (whose bus is already non-blocking) never stalls.
3. Dispatcher gains a Method::Session variant and invoke/handle_text variants taking an optional &Session; events.subscribe and events.unsubscribe are registered there and error with command.no_session when called in-process.
4. Transport: each connection gets a Session and a writer thread that owns all writing (responses and notifications both go through the outbox, so framing stays intact); the session is closed and joined when the client goes away.
5. Client: buffer notifications so invoke() still sees its response, plus next_notification/recv_notification accessors.
6. New stable codes command.no_session and command.unknown_subscription; unit tests in events/dispatch/transport plus an integration test proving a second client sees the first client's change.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
New crates/sub-command/src/events.rs holds the per-connection Session: an Outbox (bounded queue every outbound line passes through) plus a map of subscriptions, each with an EventReceiver from EngineHandle::subscribe and a pump thread that encodes ChangeEvents as events.changed notifications (params: subscription, event, lagged). The dispatcher gained a Method::Session variant and *_in variants of invoke/call/handle_call/handle_incoming/handle_value/handle_text taking an optional session; events.subscribe and events.unsubscribe are served there and answer the new stable code command.no_session in-process. A second new code, command.unknown_subscription, covers an id this connection does not hold. system.list_methods reports them with kind 'session'.

Slow-subscriber policy (AC 2): back-pressure never reaches the engine. The engine bus already drops oldest events per subscriber; on top of that the connection outbox is bounded (DEFAULT_OUTBOX_CAPACITY 4096) and a push into a full outbox closes it with a logged warning, which ends the pump threads and the reader loop so the connection is dropped. Responses now travel through the same outbox, so a new writer thread owns the socket and framing cannot interleave with pushed notifications.

Client gained notification buffering: call() sets aside any notification arriving before its response, with next_notification/recv_notification to collect them.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the local GStreamer prefix sourced); cargo test -p sub-command green (70 lib tests incl. 6 events and 3 dispatcher session tests, 3 tests/events.rs integration tests, 2 second_process, 6 doctests), tests/events.rs re-run three times for flakiness.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added event subscription to the Command API: events.subscribe returns a per-connection subscription id and streams events.changed notifications for every engine ChangeEvent, events.unsubscribe stops one (and a second attempt reports command.unknown_subscription). Subscriptions live in a per-connection Session with a bounded outbox that all outbound lines share; a client that stops reading fills it, is logged and dropped, so no back-pressure reaches the engine thread. Verified by unit tests (slow-subscriber drop, unsubscribe silence, session teardown), dispatcher tests, and tests/events.rs where a second connected client receives the change the first client made; fmt, workspace clippy -D warnings and cargo test -p sub-command all pass.
<!-- SECTION:FINAL_SUMMARY:END -->
