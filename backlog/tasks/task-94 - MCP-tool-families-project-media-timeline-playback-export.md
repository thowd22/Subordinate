---
id: TASK-94
title: 'MCP tool families: project, media, timeline, playback, export'
status: Done
assignee:
  - '@opus-task-94'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 14:14'
labels:
  - mcp
milestone: m-6
dependencies:
  - TASK-93
  - TASK-63
references:
  - docs/PLAN.md
priority: high
ordinal: 115000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The core agent surface (§7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 project.* (new, open, save, list_sequences, settings), media.* (import, list, probe, relink, make_proxy), timeline.* (add/move/trim/split/delete clip, tracks, markers, get_state), playback.* (seek, play, pause, render_frame_png), export.* (list_presets, render, progress)
- [x] #2 render_frame_png returns an image so an agent can look at the frame
- [x] #3 Each tool has a description and JSON schema; an integration test calls every tool once
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-edit: add a project.replace command (ReplaceProject) so project.new and project.open swap the open project through the ordinary undoable path.
2. sub-command/src/agent.rs: register the engine-served families on every Dispatcher — project.new/open/save/list_sequences/settings, media.list, timeline.* (aliases onto the clip/track/marker command kinds, plus timeline.get_state) and playback.play/pause/seek/status over EngineHandle's transport. Add Dispatcher alias support so an alias carries the command's own parameter schema.
3. sub-command/src/host.rs: a HostServices trait plus register_methods and an exported docs/schema/host-api.json for the families that need services the engine does not own — media.probe, media.make_proxy, playback.render_frame_png, export.list_presets, export.render, export.progress.
4. subordinate-cli: implement HostServices over sub-media (probe, proxy), sub-render + png (single-frame PNG) and sub-export (presets, export job with progress), and install it in serve.
5. subordinate-mcp: compile in host-api.json alongside the other schemas, and return playback.render_frame_png's PNG as an MCP image content block.
6. Tests: unit tests per module, a schema round-trip, and an integration test that calls every published tool once.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design: the five families are split by what can serve them.

- sub-command/src/agent.rs is installed on every Dispatcher, so project.new/open/save/list_sequences/settings, media.list, the timeline.* family and playback.play/pause/seek/status are in docs/schema/command-api.json and every build serves them. The timeline.* mutations are aliases for the registered clip/track/marker command kinds (new Method::Alias on the Dispatcher), applied through the same envelope and the same history and carrying the command's own parameter schema, so nothing is implemented twice.
- project.new and project.open apply a new sub-edit command, project.replace (ReplaceProject), whose inverse carries the outgoing project whole. Opening a file is therefore an ordinary undoable edit.
- sub-command/src/host.rs defines a Services trait for what the engine cannot serve — media.probe, media.make_proxy, playback.render_frame_png, export.list_presets/render/progress — exported as its own document, docs/schema/host-api.json, the way the plugin API already is. subordinate-cli implements it (src/host.rs) over sub-media's prober and proxy transcoder, a new render::frame_png (compositor plus a png encode, with area-averaged downscale for the width argument), and sub-export's preset library and export job run on its own thread with statuses in a table export.progress reads.
- The MCP bridge compiles the third schema in and publishes an image content block whenever a result carries base64 data, an image mime type and a pixel size, which is what makes render_frame_png something a model can look at.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test passes for sub-edit, sub-command, subordinate-cli and subordinate-mcp (34 test binaries, no failures) and for sub-model, sub-plugin and subordinate-sdk, which read the committed schemas.

AC #1 and #3 are proved by bins/subordinate-mcp/tests/tool_families.rs, which serves a real engine over the local socket with a stand-in host, exercises each of the five families with real arguments against the project (the save really writes a file, the proxy is really recorded on the item, the split really leaves two clips and undoes), and then calls every one of the 102 published tools once, requiring each to answer as a tool result — success or a structured SubError with a parseable code — rather than a protocol error. bins/subordinate-mcp/src/tools.rs also asserts every family member of docs/PLAN.md §7 is published with a description and an object parameter schema.

AC #2 is proved in the same test: playback_render_frame_png comes back with an image content block whose mime type is image/png beside the structured result.

Environment note: this machine has no GPU and no system GStreamer, so the CLI's real host services (the discoverer, the proxy transcoder, the compositor read-back and the encoder) are compiled and clippy-clean here but were exercised only through the stand-in Services. A machine with hardware should confirm playback.render_frame_png and export.render end to end; that is the same limitation the render subcommand's own test already carries.

cargo test -p sub-ui also passes (exit 0), which covers the GUI's use of the Dispatcher the new families were installed on.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The five agent tool families of docs/PLAN.md §7 are published by the MCP bridge.

The engine-served half lives in a new sub-command::agent module installed on every Dispatcher: project.new/open/save/list_sequences/settings, media.list, a timeline.* family whose eleven mutations are aliases for the registered clip, track and marker command kinds, timeline.get_state (one sequence as OTIO-shaped JSON), and playback.play/pause/seek/status over the engine's transport. Opening and starting a document go through a new undoable sub-edit command, project.replace. The half the engine cannot serve — media.probe, media.make_proxy, playback.render_frame_png and export.list_presets/render/progress — is a sub-command::host::Services trait exported as docs/schema/host-api.json and implemented by subordinate-cli over sub-media, a new render::frame_png and sub-export; subordinate-mcp compiles that third document in, so every method is a tool with the command's own description and parameter schema, and publishes a frame as an MCP image content block.

Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and the test suites of sub-edit, sub-command, subordinate-cli, subordinate-mcp, sub-model, sub-plugin and subordinate-sdk. bins/subordinate-mcp/tests/tool_families.rs drives a real engine over the socket, exercises each family against the project, checks the PNG arrives as an image block, and calls all 102 published tools once.
<!-- SECTION:FINAL_SUMMARY:END -->
