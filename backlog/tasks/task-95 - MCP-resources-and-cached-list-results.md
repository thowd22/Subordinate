---
id: TASK-95
title: MCP resources and cached list results
status: Done
assignee:
  - '@opus-task-95'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 01:02'
labels:
  - mcp
milestone: m-6
dependencies:
  - TASK-93
references:
  - docs/PLAN.md
priority: medium
ordinal: 116000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Resources let agents read project state without tool calls.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Resources: project://current, sequence://{id}, media://{id} returning OTIO-shaped JSON
- [x] #2 List results carry ttlMs per the 2026-07-28 spec; resource updates notify subscribers via the event bus
- [x] #3 Tested with the MCP inspector
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add src/resources.rs to subordinate-mcp: URI grammar (project://current, sequence://{id}, media://{id}), a Resources reader that projects the Command API's project.get result into OTIO-shaped JSON per resource, resource list + templates, and the change-event to URI mapping.
2. Declare the resources capability (with subscribe + listChanged) on the bridge and implement list_resources, list_resource_templates and read_resource; carry ttlMs and cacheScope (SEP-2549, spec 2026-07-28) on every list and read result, and on tools/list.
3. Implement resources/subscribe and resources/unsubscribe on the bridge: a watcher opens a second Command API connection, calls events.subscribe, and forwards each events.changed notification to the MCP peer as notifications/resources/updated for the subscribed URIs, plus notifications/resources/list_changed when a sequence or media item is added or removed.
4. Tests: unit tests for the URI grammar, the projection and the event mapping; a bridge test against a real engine for list/read; and an end-to-end stdio test that lists, reads, checks ttlMs, subscribes and sees an update notification arrive after an edit.
5. Verify with cargo fmt --check, clippy -D warnings and cargo test for the touched crates; attempt the MCP inspector and record whether this offline environment allows it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in bins/subordinate-mcp: new src/resources.rs (URI grammar, project.get projection, resource list and templates, ttlMs/cacheScope constants, change-event to stale-URI mapping) and src/watch.rs (a second Command API connection subscribed to the engine's event bus, fanned out to MCP subscribers over a bounded broadcast channel). bridge.rs now advertises the resources capability with subscribe and listChanged, serves resources/list, resources/templates/list and resources/read, carries ttlMs and cacheScope on tools/list, resources/list, resources/templates/list and every read, and serves both subscription mechanisms: the 2026-07-28 subscriptions/listen stream (accepted_subscription_filter + listen) and resources/subscribe for clients on earlier protocol versions.

Verification: cargo fmt --all --check passes; cargo clippy --workspace --all-targets -- -D warnings passes (5m30s, exit 0, with the local GStreamer prefix on PKG_CONFIG_PATH); cargo test -p subordinate-mcp passes 26 unit tests plus 4 bridge, 2 mcp_json, 2 stdio integration tests and the doctest. AC1 and AC2 are proven by tests/resources.rs (list, read of each of the three URI kinds against a real engine over the Command API socket, ttlMs and cacheScope on a read, mcp.unknown_resource for a URI that names nothing, and an edit arriving on the change feed as the resources it staled) and by tests/stdio.rs, which speaks raw MCP to the built binary over its stdin and stdout: resources capability with subscribe and listChanged, ttlMs plus cacheScope on tools/list, resources/list, resources/templates/list and resources/read, the project's own JSON in a read, and notifications/resources/updated arriving after a track.add on a subscribed sequence.

AC3 (MCP inspector) is left unchecked. The inspector itself is available here (npx @modelcontextprotocol/inspector starts), but driving it needs subordinate-cli built so the bridge has an editor to talk to, and subordinate-cli would not finish linking in this environment: the workspace target directory is shared with several other agents building concurrently with different RUSTFLAGS, so the build was repeatedly invalidated. The stdio acceptance test above exercises the same protocol surface the inspector would, over the real binary.

AC3 done after all: once subordinate-cli finished building, the MCP inspector CLI (npx @modelcontextprotocol/inspector --cli target/debug/subordinate-mcp, letting the bridge launch its own headless subordinate-cli) answered resources/list with project://current, resources/templates/list with sequence://{id} and media://{id}, resources/read of project://current with the project JSON plus ttlMs 5000 and cacheScope private, and tools/list with all 45 tools (schema portability: 0 errors). One caveat worth knowing: the endpoint directory must be short, or the bridge refuses with command.endpoint_unavailable because the Unix socket path exceeds 100 characters. The inspector's own CLI printer shows ttlMs and cacheScope on a read but omits them from its resources/list rendering; the stdio test asserts they are on the wire there too.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
subordinate-mcp now publishes the open project as MCP resources and tells clients when they go stale. New src/resources.rs turns one project.get round trip into project://current, sequence://{id} and media://{id} — the project file's own OTIO-shaped JSON, each carrying the revision it was read at — and maps a change event to the resource URIs it staled; new src/watch.rs takes a second Command API connection, subscribes to the engine's event bus, and fans updates out over a bounded broadcast channel that can never slow the engine down. bridge.rs advertises the resources capability with subscribe and listChanged, serves resources/list, resources/templates/list and resources/read, carries the 2026-07-28 ttlMs and cacheScope hints (SEP-2549) on every cacheable list and read including tools/list, and serves both subscription mechanisms: the 2026-07-28 subscriptions/listen stream and resources/subscribe for clients on earlier protocol versions. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p subordinate-mcp: new tests/resources.rs drives a real engine over the socket (list, read of each URI kind, ttlMs and cacheScope, mcp.unknown_resource, and an edit arriving on the change feed), extended tests/stdio.rs speaks raw MCP to the built binary and sees notifications/resources/updated after an edit on a subscribed sequence, and the MCP inspector CLI was run against the binary for resources/list, resources/templates/list, resources/read and tools/list.
<!-- SECTION:FINAL_SUMMARY:END -->
