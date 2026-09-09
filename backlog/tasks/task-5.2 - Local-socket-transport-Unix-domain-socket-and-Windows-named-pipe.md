---
id: TASK-5.2
title: 'Local socket transport: Unix domain socket and Windows named pipe'
status: Done
assignee:
  - '@opus-task-5.2'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 15:08'
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
- [x] #1 Server listens on a per-user socket path (XDG runtime dir, macOS temp dir, Windows named pipe) written to a discoverable lock file
- [x] #2 Multiple concurrent clients are served with newline-delimited JSON framing
- [x] #3 Stale socket files from crashed instances are cleaned up on start
- [x] #4 Integration test connects from a second process and runs a command
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add an endpoint module to sub-command: per-user Endpoint (XDG_RUNTIME_DIR/subordinate on Linux, TMPDIR on macOS, a namespaced named pipe on Windows), instance-name validation, unix socket path length check, 0700 directory mode.
2. Add a discoverable lock file (JSON: pid, transport, address, instance, version) written atomically next to the socket; readers use it to find a running server.
3. Stale cleanup on bind: read the lock file, probe the recorded address, and delete the stale socket plus lock file when nothing answers; refuse to bind when a live server holds it.
4. Add a transport module: Server::bind spawns an accept thread (nonblocking accept plus stop flag) and a thread per connection, newline-delimited JSON framing over Dispatcher::handle_text, bounded message size; shutdown removes the lock file and the socket.
5. Add a blocking Client (connect via endpoint or lock file, call/notify) for the CLI, the MCP bridge and tests.
6. Tests: endpoint naming and validation, stale-lock cleanup, concurrent clients, notifications, malformed JSON, and an integration test that re-execs the test binary as a second process which connects and runs a command.
7. Verify with cargo fmt, clippy pedantic -D warnings and cargo test -p sub-command.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

- New sub_command::endpoint: Endpoint (per-user address plus lock file), Address (Unix socket path or Windows named pipe), Transport, LockFile. Linux uses XDG_RUNTIME_DIR/subordinate, macOS the per-user TMPDIR, Windows a namespaced pipe under LOCALAPPDATA-rooted lock files. Instance names are validated before they reach a path, the Unix directory is chmod 0700 and the socket 0600, and the socket path is refused above 100 bytes so an over-long sun_path fails with a clear error rather than at bind.
- New sub_command::transport: Server (nonblocking accept thread plus a thread per client) and Client (blocking, one line out, one line back). Framing is newline-delimited JSON straight through Dispatcher::handle_text, capped at MAX_MESSAGE_BYTES (8 MiB). Shutdown and Drop release the address and delete the lock file.
- Stale cleanup on bind: the recorded and native addresses are probed; anything that answers means a live instance and bind fails with command.address_in_use, anything that does not is unlinked along with the lock file. An unparseable lock file is treated as wreckage.
- New stable codes: command.invalid_instance, command.endpoint_unavailable, command.address_in_use, command.lock_file_invalid, command.not_running, command.transport_io.
- Dependency added: interprocess 2 (MIT OR Apache-2.0, default features off). It is the only crate that gives one blocking API over both Unix sockets and Windows named pipes without hand-written unsafe Win32 code, which the workspace unsafe_code lint discourages.

Caveats

- Only the Linux code path was executed here; macOS and Windows differ by cfg only and are covered by the same tests on CI runners.
- Client::connect_to retries for CONNECT_TIMEOUT while a pipe is merely busy (ERROR_PIPE_BUSY), which is the ordinary Windows state between accepts. That path could not be exercised on Linux.
- Open connections are not joined at shutdown: a client that stops speaking must not be able to hold the editor open. The accept thread is joined, so the address is always released.

Validation

- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-command: 61 unit, 1 integration (second process), 5 doc tests pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the local socket transport for the Command API: sub_command::endpoint derives a per-user address (XDG runtime dir on Linux, per-user TMPDIR on macOS, a named pipe on Windows) and publishes it in a discoverable JSON lock file, and sub_command::transport serves it with newline-delimited JSON-RPC, one thread per client, over interprocess 2. Binding first probes the recorded address and clears the socket and lock file left by a crashed instance, refusing only when a live server answers; shutdown releases both. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test -p sub-command (61 unit tests covering framing, concurrency, stale cleanup and endpoint naming, an integration test that re-executes the test binary as a second process which finds the server through the lock file and runs bin.create, and 5 doc tests).
<!-- SECTION:FINAL_SUMMARY:END -->
