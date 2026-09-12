---
id: TASK-141
title: >-
  The editor must bind the Command API socket so the MCP bridge drives the
  running GUI, not a headless CLI
status: In Progress
assignee:
  - '@opus-task-141'
created_date: '2026-09-12 01:22'
updated_date: '2026-09-12 01:56'
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
- [x] #1 SubordinateApp binds the Command API socket on startup (Unix socket or named pipe per platform) and writes the discoverable endpoint; a second instance refuses or takes over per a documented rule
- [x] #2 subordinate-mcp connects to the running editor when one is up and only falls back to subordinate-cli serve when none is; the choice is logged and exposed in the MCP server's info
- [x] #3 An interaction test starts the assembled app, connects a Command API client, adds a clip, and asserts the timeline panel shows it on the next frame and that Undo in the app removes it
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Implementation

- crates/sub-ui/src/command_api.rs (new). CommandApi owns the Endpoint and the sub_command::transport::Server bound over the session's dispatcher. serve() spawns a sub-ui-command-api thread that calls Server::bind and posts the result down a channel; sync(&session), called once a frame from SubordinateApp::ui, collects it. Nothing about the socket runs on the UI thread: the bind is off-thread, the server accepts on its own thread and serves each client on another, and every command those threads dispatch is queued to the engine thread like a panel's. The UI thread's only part is the EditorSession::poll it already did.
- Routing through the same session. The server serves EditorSession's own Arc<Dispatcher>, so a socket command is applied on the engine the panels draw: it lands in the same undo history and reaches the panels through the change events they already read. Opening a project replaces the engine (and the dispatcher with it), so EditorSession now counts generations and CommandApi rebinds the same endpoint whenever the generation moves - otherwise an agent would be editing a project nobody is looking at.
- Single-instance rule (documented in command_api.rs, docs/mcp-guide.md and docs/DEVELOPMENT.md). It is the rule transport::clear_stale already implements, now surfaced rather than invented: the editor asks its address, and the address its lock file records, whether anything answers. Something answers -> another editor is live, this one refuses (command.address_in_use, logged with the other pid), keeps running with no agent surface, and CommandApi::refusal() carries the reason; nothing answers -> the socket and lock file are a corpse, and the new editor takes over. A refusal is not retried every frame (a live neighbour would make that a spin); the next bind - i.e. the next project opened - tries again. subordinate --instance NAME gives a second window an endpoint of its own.
- Exit. eframe::App::on_exit calls CommandApi::shutdown, and Server's own Drop unlinks the socket and removes the lock file, so a clean exit leaves nothing behind.
- Options. AppOptions gains serve_command_api, instance and endpoint_dir. from_env() turns serving on (reading SUBORDINATE_INSTANCE, SUBORDINATE_ENDPOINT_DIR and SUBORDINATE_NO_COMMAND_API), so the real binary serves; AppOptions::default() leaves it off, so an app embedded in a test does not reach for the per-user endpoint a developer's editor may be holding. The subordinate binary gains --instance NAME and --no-command-api.
- Ready line. The ui-smoke ready line now carries command_api=<address> | refused:<code> | off, and is not printed until the endpoint has settled, so a CI step that connects the moment it appears cannot race the bind.
- subordinate-mcp. The backend already preferred a running editor; the choice is now stated in the MCP server's instructions as well as the log ('Connected to the editor already running at <address>...' / 'No editor was listening, so this session drives a headless engine of its own...'). scripts/mcp-roundtrip.py gains --require-editor (fails unless the bridge reached a real editor) and --max-chars.

## Validation

- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-ui: all green, including the new interaction test a_client_on_the_editors_socket_adds_a_clip_the_window_shows_and_undoes (AC 3) - the assembled app on lavapipe, a sub_command::transport::Client connected to the endpoint the window published, clip.add over the socket, the clip present in TimelinePanel::layouts() (the index the panel paints from) on the next frame, then Undo clicked in the window's Edit menu and the clip gone from both the panel and project.get. Four unit tests in command_api.rs cover AC 1: the endpoint serves and the lock file goes away on shutdown, a second editor refuses with command.address_in_use while the first keeps serving, a stale lock file is taken over, and the socket follows the project that is opened.
- cargo test -p sub-command -p subordinate-mcp: all green, including the two bridge tests that now assert the connection note (AC 2): a running editor gives 'Connected to the editor already running at <address>', a launched one gives 'No editor was listening'.
- cargo test -p subordinate: the new --instance / --no-command-api argument tests pass.

## AC 4: what the supervisor has to verify

Left unchecked - it needs the GPU desktop runner.

.github/workflows/gpu-smoke.yml's nvidia-desktop-linux job now: names SUBORDINATE_ENDPOINT_DIR at job level so the editor's step and the bridge's step land on one socket (XDG_RUNTIME_DIR is not reliably shared between step shells); sets SUBORDINATE_MCP_NO_LAUNCH=1 so the bridge cannot fall back to a headless engine; fails the editor step unless the ready line reports a bound command_api=; and runs the round-trip as timeline.get_state -> sequence.rename -> timeline.get_state on the sample project's own committed sequence id (0192f3a0-0006-7000-8000-000000000001, the Main whose clips are in the screenshot), with --require-editor. A headless subordinate-cli serve could not answer that at all: it starts on an empty project and refuses with 'the project has no sequences'. A second screenshot is taken after the agent's edit.

The job cannot pass on the current image: ami-05c99b3a15c9d2fef bakes release v0.1.1, whose subordinate does not bind the endpoint. AC 4 needs a desktop image (or release) carrying this change - infra/images/linux-desktop/deploy.sh --run --wait - and then a GPU smoke run of nvidia-desktop-linux.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The editor now binds the Command API endpoint itself, so subordinate-mcp drives the running GUI instead of a headless subordinate-cli serve (TASK-137's finding).

crates/sub-ui/src/command_api.rs owns the endpoint and a transport::Server over EditorSession's own Arc<Dispatcher>: the bind runs on a thread of its own and is collected at the next frame, so the socket never touches the UI thread, and commands that arrive on it are applied to the engine the panels draw - same undo history, visible on the next frame through the change events the session already polls. Opening a project replaces the engine, so the session counts generations and the server is rebound on the same endpoint when it moves; on_exit and Server's Drop release the address and the lock file. The single-instance rule is the one clear_stale already applies, now documented and surfaced: refuse (command.address_in_use) when a live editor answers, take over a lock file nothing answers, and subordinate --instance NAME for an endpoint of your own. The MCP bridge, which already preferred a running editor, now names which it got in its MCP instructions as well as its log.

Verified by cargo test -p sub-ui, including a new interaction test that connects a real transport::Client to the assembled app's published endpoint, adds a clip with clip.add, finds it in the timeline panel's own layout on the next frame and sees it removed by Undo in the window's Edit menu; four command_api unit tests cover the bind, the refusal, the stale takeover and the rebind; two subordinate-mcp bridge tests assert the connection note. fmt and clippy clean across the workspace. AC 4 is unchecked: the desktop smoke job now targets the GUI's endpoint with no launch fallback, but it needs a desktop image carrying a build with this change.
<!-- SECTION:FINAL_SUMMARY:END -->
