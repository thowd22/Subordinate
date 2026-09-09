---
id: TASK-93
title: 'subordinate-mcp: rmcp stdio bridge to the Command API'
status: Done
assignee:
  - '@opus-task-93'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 23:32'
labels:
  - mcp
milestone: m-6
dependencies:
  - TASK-5.4
  - TASK-6
references:
  - docs/PLAN.md
priority: high
ordinal: 114000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The MCP server is how Claude Code drives the editor (§7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Binary speaks MCP over stdio using rmcp, connects to the running app's socket, or launches subordinate-cli serve when none is running
- [x] #2 .mcp.json entry for Claude Code is documented and committed as an example
- [x] #3 Tools are generated from docs/schema/command-api.json so names and descriptions match
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Turn bins/subordinate-mcp into lib+bin so the bridge is testable; add rmcp 3 (server, transport-io) and tokio deps.
2. tools.rs: parse the committed docs/schema/command-api.json (include_str!) into MCP tools - one per Command API method, description taken verbatim from the schema, input schema inlined from the method's params $ref with the document's $defs attached; MCP-safe tool name (dots to underscores) with a name->method table.
3. backend.rs: find the running editor through the endpoint lock file and connect with sub_command::transport::Client; when nothing is listening, launch 'subordinate-cli serve' next to our own executable, read its readiness line and connect to the announced address; close its stdin on drop.
4. bridge.rs: rmcp ServerHandler with get_info/list_tools/call_tool; call_tool forwards params to the Command API over the blocking client on a blocking task, returns the JSON-RPC result as structured content, and turns a SubError into a tool-level error carrying its stable code.
5. main.rs: init logging on stderr, serve the bridge over rmcp stdio in a current-thread tokio runtime.
6. Commit .mcp.json at the repository root as the Claude Code example and document it in docs/PLAN.md ss7 / README.
7. Tests: unit tests for tool generation and name mapping and readiness parsing; integration test that runs the bridge against an in-process Command API server and against a launched subordinate-cli serve.
8. Verify with cargo fmt --check, clippy -D warnings and cargo test for the touched crates.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented as a lib+bin package (bins/subordinate-mcp): tools.rs generates one MCP tool per Command API method from the compiled-in docs/schema/command-api.json (description and parameter schema verbatim; dots in method names become underscores in tool names because MCP clients only accept [A-Za-z0-9_-], and the tool title keeps the method name); backend.rs connects through the endpoint lock file with sub_command::transport::Client and otherwise launches 'subordinate-cli serve --compact', reads its readiness line and connects to the announced address, holding the child's stdout open and closing its stdin on drop for a clean shutdown; bridge.rs is the rmcp ServerHandler (get_info/list_tools/call_tool), forwarding calls on a blocking task and returning engine failures as tool errors carrying the JSON SubError with its stable code; main.rs serves it over rmcp stdio with logging on stderr. New error codes mcp.schema_invalid, mcp.cli_not_found, mcp.launch_failed, mcp.session_failed. Configuration is by environment (SUBORDINATE_INSTANCE, SUBORDINATE_ENDPOINT_DIR, SUBORDINATE_CLI, SUBORDINATE_MCP_NO_LAUNCH), because an MCP client passes env, not arguments.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (exit 0); cargo test -p subordinate-mcp passes 22 tests. AC1 is proven by tests/stdio.rs, which runs the built binary as a child process, completes the MCP initialize handshake on its stdin/stdout, lists 45 tools and calls bin_create, and sees the bin appear in an engine reached over the socket; tests/bridge.rs covers the launch path (starting subordinate-cli serve, reading its readiness line, calling project.revision and shutting the child down so no lock file is left) and the command.not_running / mcp.launch_failed failures. AC2: docs/examples/mcp.json is committed and documented in docs/DEVELOPMENT.md ('Driving the editor from an agent (MCP)') and README.md; tests/mcp_json.rs keeps it honest against the binary name and the environment variables the bridge reads. The repository's own .mcp.json was left alone because it registers the Backlog MCP server. AC3: tools::ToolSet is generated from the compiled-in docs/schema/command-api.json; tests assert every method becomes a tool with the schema's own description, parameter schema and $defs, and the stdio test checks bin_create's description over the wire.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
subordinate-mcp is now an rmcp 3.2 stdio MCP server bridging to the Command API. It offers one tool per Command API method, generated from the committed docs/schema/command-api.json (same descriptions and parameter schemas; dots become underscores because MCP tool names may not contain dots, with the method name kept as the tool title), connects to a running editor through the endpoint lock file and otherwise launches 'subordinate-cli serve' and connects to the address its readiness line announces, stopping that child cleanly when the session ends. Engine failures come back as tool errors carrying the JSON SubError and its stable code. A .mcp.json example is committed at docs/examples/mcp.json and documented in docs/DEVELOPMENT.md and README.md. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p subordinate-mcp (22 tests), including an end-to-end test that drives the real binary over stdio through the MCP handshake, tools/list and tools/call.
<!-- SECTION:FINAL_SUMMARY:END -->
