---
id: TASK-155
title: 'The running editor serves no export, probe or frame methods on its Command API'
status: To Do
assignee: []
created_date: '2026-09-12 04:44'
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
- [ ] #1 A running editor answers export.list_presets, export.render and export.progress, and the export it starts appears in the window export panel with its progress and its result
- [ ] #2 The export runs on the window own render context and job pool - not a second headless engine - and the window keeps painting while it does
- [ ] #3 media.probe, media.make_proxy and playback.render_frame_png are answered by the running editor too, so system.list_methods is the same list from the window and from subordinate-cli serve
- [ ] #4 An egui_kittest test drives an export over the socket and asserts the panel shows it, and the export matrix GUI-versus-CLI comparison can run against the editor on the desktop image
<!-- AC:END -->
