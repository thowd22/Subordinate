---
id: TASK-127
title: >-
  Wire the engine into SubordinateApp so panels apply commands instead of
  logging them
status: Done
assignee:
  - '@opus-task-127'
created_date: '2026-09-10 19:43'
updated_date: '2026-09-11 00:06'
labels:
  - ui
  - core
milestone: m-2
dependencies:
  - TASK-12
  - TASK-43
  - TASK-5.1
references:
  - docs/PLAN.md
priority: high
ordinal: 147000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Several merged UI tasks report the same gap: the assembled app shell (SubordinateApp in sub-ui) has no engine handle, so panel actions are logged rather than applied. TASK-111 notes insert/overwrite only logs the planned command, TASK-40 marker actions reach app.rs as log lines, TASK-71 says there is no file-open command or autosave worker instance because the app has no engine handle, and TASK-37's viewer half is blocked on the same wiring. Each panel is tested in isolation through the kittest harness, but the real app cannot edit a project. This is the glue that makes the UI functional and it must exist before end-to-end UI tests, the Xvfb smoke and any user trial mean anything.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 SubordinateApp owns an engine handle (sub-edit engine thread) and a Command API client; project open, save and autosave run through it
- [x] #2 Timeline, bin, inspector, markers, track headers and sequence tabs dispatch their commands through the handle; the log-only placeholders in app.rs are removed
- [x] #3 Engine change events refresh the panels (subscription wired) so an edit made via the MCP bridge appears in the running UI
- [x] #4 An interaction test opens the sample project in the assembled app, splits a clip via the timeline, undoes it via the menu, and asserts project state through the Command API
- [x] #5 The Xvfb window smoke shows the sample project loaded through the real engine path
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add an EditorSession module to sub-ui that owns a sub_edit::Engine, the project snapshot Arc, the active sequence id, the autosave worker and a change-event subscription; open/save/save-as and undo/redo go through it.
2. Add sub-command as a dev-dependency of sub-ui so tests can assert project state through the Command API dispatcher.
3. Give the ui action types a uniform way to become BoxedCommand groups (MoveGroup/SplitGroup/TrimGroup commands(), fade/transition/inspector/source-edit/marker/track/bin/sequence-tab conversions) and add EditorSession::apply_group.
4. Rewrite SubordinateApp around the session: hold no owned Project/Sequence, dispatch every panel outcome through the engine handle, poll the change-event subscription each frame so an MCP edit refreshes the panels, add File (Open/Save/Save As) and Edit (Undo/Redo) menus, and a sequence-tab strip.
5. Delete log_timeline_edits and every other 'not wired up yet' placeholder in app.rs.
6. Add crates/sub-ui/tests/app_engine.rs: build SubordinateApp through egui_kittest build_eframe, open the sample project, split a clip on the timeline, undo via the Edit menu, and assert the project through sub_command::Dispatcher.
7. Verify with fmt, clippy pedantic and cargo test -p sub-ui under the GStreamer env; report the Xvfb smoke criterion as unverifiable here (no Xvfb/ImageMagick in this environment).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

- New crates/sub-ui/src/session.rs: EditorSession owns the sub-edit Engine thread, the immutable Arc<Project> snapshot the panels read, the revision, a ChangeEvent subscription, a sub_command::Dispatcher over the same handle (the in-process Command API client), the autosave worker and the open project file. apply/apply_boxed/apply_group/begin_group/commit_group/undo/redo/open/adopt/save/save_as/poll live there. A group whose command fails is aborted, not committed, so a half-applied drag cannot reach the undo stack.
- SubordinateApp no longer owns a Project. It holds the session plus a cached clone of the active Sequence, refreshed only when (revision, active sequence) moves. dock_ui reads one Arc snapshot for the whole frame, collects the panels' outcomes into a FrameEdits struct, and applies them after the dock has given its borrows back; that is what keeps one gesture one undo entry.
- Dispatch wired: timeline clip move/trim/split/fade/crossfade/bin-drop, track-header actions (with the panel's begin/commit group brackets), marker actions, inspector gestures, media-bin folder commands, sequence tab create/rename/delete, insert/overwrite from the bin, and Ctrl+Z / Ctrl+Y. log_timeline_edits and every 'not wired up yet' placeholder in app.rs are gone.
- New UI: a File menu (Open/Save/Save As over the existing snapshot history), an Edit menu drawn from the engine's HistorySummary through HistoryList::from_summary, and a sequence tab strip between the menu bar and the dock.
- SubordinateApp::new now returns SubResult<Self> rather than Result<Self, RenderError>, because starting the engine can fail; RenderError is lifted into SubError keeping its own stable code.
- sub-command moved from dev-dependencies to dependencies of sub-ui.

Decisions

- Opening a project replaces the engine rather than mutating it: the engine owns the project for its whole life, and a closed project's undo stack goes with it. Snapshot restore and autosave recovery take the same path.
- HistorySummary carries only the two labels either side of the cursor, so HistoryList::from_summary names the rest with EARLIER_STEP_LABEL/LATER_STEP_LABEL. The Edit menu only ever shows the two real ones.
- MediaBinAction::Import and ::Relink return None from into_command: they are jobs (hash, probe, search) that end in a command, not commands themselves, and this window does not host the import queue or the relink dialog yet. They are logged as unhosted rather than as unwired.

Validation

- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-ui: 491 tests pass across 28 binaries, including every committed snapshot. cargo test -p sub-ui --lib after the session unit tests were added: 317 pass.
- crates/sub-ui/tests/app_engine.rs builds the real SubordinateApp through egui_kittest's build_eframe on the lavapipe adapter and covers AC 4 and AC 3: Ctrl+K on the timeline cuts a clip of the sample project, the Edit menu's Undo entry rejoins it, and both are read back through the window's own Dispatcher via project.get; a RenameSequence applied straight on the engine handle (the MCP path) shows up in the panels on the next frame.
- Six unit tests in session.rs cover AC 1's save and autosave halves: save with no file refuses with ui.project_unsaved, save_as writes and starts the autosave worker, a second save goes to the same file, open replaces the engine and its history, and an aborted group leaves the project untouched.

Not verified here

- AC 5 (the Xvfb window smoke) cannot run in this environment: Xvfb, xdpyinfo, xwd and ImageMagick are all absent. The ready line now also reports sequences=, tracks= and revision= read out of the engine, and scripts/ui-smoke.sh fails the run when either count is zero, so a window that came up on an empty project is no longer photographed as a pass. That path needs a CI run to prove.

2026-09-10 supervisor verification: CI run 34539791304 ui-smoke artifact app.log shows 'opened .../sample-project.sub' followed by 'ui-smoke ready: frames=3 popout=true popout_frames=1 project=loaded sequences=2 tracks=3 revision=0', i.e. the sample project is loaded through the engine (revision reported by the engine handle) in the assembled app under Xvfb.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
SubordinateApp now owns the engine handle and Command API client; panels dispatch commands through it, engine events refresh panels, and the assembled app opens the sample project through the real engine path (verified by tests and the CI Xvfb smoke log).
<!-- SECTION:FINAL_SUMMARY:END -->
