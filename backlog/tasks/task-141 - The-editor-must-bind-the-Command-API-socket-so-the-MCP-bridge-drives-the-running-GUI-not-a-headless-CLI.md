---
id: TASK-141
title: >-
  The editor must bind the Command API socket so the MCP bridge drives the
  running GUI, not a headless CLI
status: In Progress
assignee:
  - '@opus-task-141'
created_date: '2026-09-12 01:22'
updated_date: '2026-09-12 01:25'
labels:
  - ui
  - core
  - mcp
milestone: m-2
dependencies:
  - TASK-127
  - TASK-93
  - TASK-5.2
priority: high
ordinal: 161000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-137's desktop smoke found that sub-ui builds a Dispatcher but nothing calls Server::bind, so subordinate-mcp cannot reach the editor process and instead launches subordinate-cli serve. Edits made by an agent therefore never appear in the window the user is looking at, which contradicts docs/PLAN.md §4 and §7 (one Command API serving GUI, CLI, MCP and plugins; MCP edits appear live). The engine, dispatcher, socket server (TASK-5.2), event subscription (TASK-5.3) and the app's engine wiring (TASK-127) all exist; the missing piece is the app binding the local socket at startup, publishing its endpoint in the discoverable lock file the bridge already reads, and routing bridge commands through the same session so they land in the undo history and refresh the panels.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 SubordinateApp binds the Command API socket on startup (Unix socket or named pipe per platform) and writes the discoverable endpoint; a second instance refuses or takes over per a documented rule
- [ ] #2 subordinate-mcp connects to the running editor when one is up and only falls back to subordinate-cli serve when none is; the choice is logged and exposed in the MCP server's info
- [ ] #3 An interaction test starts the assembled app, connects a Command API client, adds a clip, and asserts the timeline panel shows it on the next frame and that Undo in the app removes it
- [ ] #4 The Linux desktop smoke job's MCP round-trip runs against the GUI process (timeline.get_state reflects a sequence visible in the screenshot)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-ui gains a CommandApi owner (crates/sub-ui/src/command_api.rs): it holds the Endpoint, binds sub_command::transport::Server on a worker thread (never the UI thread) over an Arc<Dispatcher> taken from the EditorSession, and is polled once a frame for the outcome. Dropping it removes the socket and the lock file.
2. EditorSession holds its Dispatcher as Arc and gains a generation counter bumped by adopt(), because opening a project replaces the engine; the CommandApi rebinds on the same endpoint when the generation moves, so the socket always fronts the engine the panels are drawing.
3. Single-instance rule: Server::bind already refuses a live endpoint (command.address_in_use) and takes over a stale one (clear_stale). The editor keeps running when it is refused, logs it and shows it in the Diagnostics panel; the rule is documented in docs/mcp-guide.md and DEVELOPMENT.md.
4. AppOptions gains serve_command_api/instance/endpoint_dir; from_env() turns serving on for the real binary (SUBORDINATE_INSTANCE, SUBORDINATE_ENDPOINT_DIR, SUBORDINATE_NO_COMMAND_API) and the GUI binary gains --instance/--no-command-api. An embedded app (tests) opts in explicitly.
5. subordinate-mcp: the backend already prefers a running editor; name the choice in the MCP server info (instructions) and keep the log line, so a client can see whether it is driving the GUI or a headless engine.
6. Interaction test crates/sub-ui/tests/app_engine.rs: build the assembled app serving a private endpoint directory, connect a sub_command::transport::Client, clip.add a clip, step a frame and assert the timeline panel shows it, then press Undo in the app and assert it is gone.
7. Docs: docs/mcp-guide.md and docs/DEVELOPMENT.md; gpu-smoke.yml's nvidia-desktop-linux MCP step targets the GUI's endpoint (no launch fallback) and reads back the sequence the screenshot shows.
<!-- SECTION:PLAN:END -->
