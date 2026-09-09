---
id: TASK-5.2
title: 'Local socket transport: Unix domain socket and Windows named pipe'
status: In Progress
assignee:
  - '@opus-task-5.2'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 14:52'
labels:
  - core
  - mcp
milestone: m-0
dependencies:
  - TASK-5.1
references:
  - docs/PLAN.md
parent_task_id: TASK-5
priority: high
ordinal: 31000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Out-of-process clients such as the MCP bridge need a local, authenticated channel that works on all three OSes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Server listens on a per-user socket path (XDG runtime dir, macOS temp dir, Windows named pipe) written to a discoverable lock file
- [ ] #2 Multiple concurrent clients are served with newline-delimited JSON framing
- [ ] #3 Stale socket files from crashed instances are cleaned up on start
- [ ] #4 Integration test connects from a second process and runs a command
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add an `endpoint` module to sub-command: per-user Endpoint (XDG_RUNTIME_DIR/subordinate on Linux, TMPDIR on macOS, a namespaced named pipe on Windows), instance-name validation, unix socket path length check, 0700 directory mode.
2. Add a discoverable lock file (JSON: pid, transport, address, instance, version) written atomically next to the socket; readers use it to find a running server.
3. Stale cleanup on bind: read the lock file, probe the recorded address, and delete the stale socket plus lock file when nothing answers; refuse to bind when a live server holds it.
4. Add a `transport` module: Server::bind spawns an accept thread (nonblocking accept plus stop flag) and a thread per connection, newline-delimited JSON framing over Dispatcher::handle_text, bounded message size; shutdown removes the lock file and the socket.
5. Add a blocking Client (connect via endpoint or lock file, call/notify) for the CLI, the MCP bridge and tests.
6. Tests: endpoint naming and validation, stale-lock cleanup, concurrent clients, notifications, malformed JSON, and an integration test that re-execs the test binary as a second process which connects and runs a command.
7. Verify with cargo fmt, clippy pedantic -D warnings and cargo test -p sub-command.
<!-- SECTION:PLAN:END -->
