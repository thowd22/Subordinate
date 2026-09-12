---
id: TASK-155
title: 'The running editor serves no export, probe or frame methods on its Command API'
status: In Progress
assignee:
  - '@codex'
created_date: '2026-09-12 04:44'
updated_date: '2026-09-12 18:09'
labels:
  - ui
  - api
  - export
  - bug
dependencies: []
ordinal: 167000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The editor binds the Command API at startup and an agent finds it through the lock file (TASK-141), but the dispatcher it serves is Dispatcher::new(engine) and nothing else: crates/sub-ui/src/session.rs registers the engine family - project, timeline, playback, media.list, history - and never sub_command::host::register_methods. So export.render, export.progress, export.list_presets, media.probe, media.make_proxy and playback.render_frame_png reach a running window with command.unknown_method, while subordinate-cli serve answers all six. An agent can rename a sequence in the window it can see and then has to start a second headless engine to export it, which is the opposite of what decision-7 and PLAN section 7 promise, and it is why TASK-143 could not measure the GUI export path on the desktop image at all - the only way in would have been the socket. The GUI needs its own Services over the window renderer and its ExportRunner rather than a generic one: an export asked for over the socket should run on the window job pool, show in the export panel, and read its pixels from the same compositor the viewer draws with.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A running editor answers export.list_presets, export.render and export.progress, and the export it starts appears in the window export panel with its progress and its result
- [x] #2 The export runs on the window own render context and job pool - not a second headless engine - and the window keeps painting while it does
- [x] #3 media.probe, media.make_proxy and playback.render_frame_png are answered by the running editor too, so system.list_methods is the same list from the window and from subordinate-cli serve
- [ ] #4 An egui_kittest test drives an export over the socket and asserts the panel shows it, and the export matrix GUI-versus-CLI comparison can run against the editor on the desktop image
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Lift the pieces both front ends need out of subordinate-cli so the GUI does not copy them: base64/FrameImage::png into sub_command::host, media_info_json into sub_media::probe, the project-proxy glue into sub_media::proxy, preset_json into sub_export::presets, and frame_png/downscale/encode_png into sub_export::sequence taking the caller's RenderContext. The CLI's CliServices and render::frame_png become thin callers of those.
2. Add crates/sub-ui/src/host_services.rs: GuiHost (shared state: project dir, an export request queue, the export status table) and GuiServices implementing sub_command::host::Services over the window's own RenderContext and preset library. probe, make_proxy and render_frame_png run on the calling socket thread against the window's shared wgpu device; export.render validates the preset and sequence, allocates a job id, queues the request and returns at once, so no socket thread ever blocks on the UI thread.
3. EditorSession keeps the Services and registers sub_command::host::register_methods on its dispatcher, rebuilding it with the host methods whenever a project is adopted, so the socket and the in-process dispatcher serve the same list as subordinate-cli serve.
4. SubordinateApp installs GuiServices before it binds the endpoint, pumps the queued export requests once a frame through the panel's own request() and the window's ExportRunner on the window job pool, and publishes the panel's status back into the table export.progress reads.
5. Tests: unit tests for the host bridge's queue and status mapping, and crates/sub-ui/tests/command_api_host.rs, an egui_kittest test that drives the running window over its socket - system.list_methods, export.list_presets, export.render - and asserts the export shows in the panel and export.progress follows it, skipping with a reason where the machine has no adapter or no encoder.
6. Verify: cargo fmt, clippy -D warnings, tests for the touched crates (with env-gst.sh sourced for the GStreamer crates).

7. Resume inherited implementation: audit concurrency/project adoption, add missing socket-driven egui regression, run targeted local validation and commit reviewable code and evidence.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Resumed inherited implementation. Fixed non-atomic socket reservation, initialized media project directory before endpoint bind, and prevented previous terminal panel state from completing the next queued job. Added concurrent-client unit coverage and assembled egui socket regression (two exports, busy refusal, panel/API completion, method discovery, frame PNG, probe and missing proxy). Local compilation uses /home/admin2/.cache/subordinate/env-gst.sh; no cloud resources used.

Completed GUI Services registration, shared CLI/GUI serializers, proxy helper and frame PNG compositor. Queued exports retain their accepted Arc<Project> and media directory, use the window runner/job pool, and publish the panel status. Project new/open/save notify host services synchronously; GUI remembers directories by project ID, so opening another folder before the next UI frame cannot misroute media. Also corrected system.list_methods: its prior startup snapshot omitted subsequently registered agent/host methods; discovery now reads the live method table.

Validation: local egui command_api_host regression PASSED with an actual GPU adapter and x264 encoding (no early-return skip): two socket exports, panel/progress/result agreement, continued UI painting, PNG frame, media probe, proxy missing-item refusal, busy rejection, project.new between enqueue and pump, immediate media-ID probe after opening another folder, second export and dispatcher rebuild. Seven host-bridge unit tests passed including concurrent-client reservation. Full sub-command suite passed (92 unit, 7 integration, 7 doc tests; child helper ignored as intended), then focused discovery coverage passed after adding the 93rd unit test. cargo clippy -p sub-ui -p subordinate-cli --all-targets --offline -- -D warnings, cargo fmt --all --check and git diff --check passed. GStreamer environment: source /home/admin2/.cache/subordinate/env-gst.sh. Unix socket suites required execution outside the filesystem/network sandbox; no AWS instances/workflows used.

AC4 remains unchecked only because the desktop-image GUI-versus-CLI export matrix was not run; its requested egui socket regression is implemented and passed locally. Existing session limitation: socket project.open/save does not update EditorSession.project_file (GUI Save/autosave/title), although the host media/export directory now updates immediately and safely. Direct low-level project.replace of an unknown project requires open/save to establish its host file context.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Implemented the running-window host API and shared rendering/media helpers; validated with real local egui socket exports, host concurrency tests, command/schema/transport suites, formatting and strict clippy. Desktop-image export matrix verification remains for integration.
<!-- SECTION:FINAL_SUMMARY:END -->
