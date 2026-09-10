---
id: TASK-67
title: Pop-out viewer as an egui deferred viewport sharing the compositor texture
status: Done
assignee:
  - '@opus-task-67'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 10:22'
labels:
  - ui
milestone: m-5
dependencies:
  - TASK-22
  - TASK-43
  - TASK-7
references:
  - docs/PLAN.md
priority: high
ordinal: 88000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Preview on a second display is an MVP requirement (§2). The spike proved the approach; this makes it a feature.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Viewer menu item pops the viewer into its own OS window showing the same texture with no extra render pass
- [x] #2 Closing the pop-out returns the viewer to the dock
- [x] #3 Pop-out receives playback shortcuts
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the pop-out: a snapshot of the viewer content painted through the harness, and an interaction test for the pop-out toggle. The deferred viewport itself is verified by TASK-118 on real hardware
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New sub-ui module `popout`: `PopoutShared` (Arc-shared texture handle, queued playback actions, close flag, paint counter) and `PopoutViewer` (open/close/toggle, `show` calling ctx.show_viewport_deferred with a Send+Sync closure that paints the same egui::TextureId the dock paints, no extra render pass).
2. Reuse the viewer's picture painting: lift `ViewerPanel::picture_ui` into a shared `viewer::paint_picture` used by both the docked panel and the pop-out closure.
3. Pop-out keyboard: the closure polls the editor's ShortcutMap in the pop-out viewport's own input, keeps only viewer/transport actions and queues them for the main pass, which applies them to the playhead and the PlaybackScheduler; request a repaint of the root viewport so they take effect at once.
4. Closing: the closure watches its viewport's close_requested and sets the shared close flag; the main pass clears `open`, so the picture returns to the docked viewer panel (which shows a 'popped out' placeholder while it is out).
5. Viewer menu item (`popout_menu_ui`) in the app's menu bar, plus app wiring: publish the composited ViewerFrame to the pop-out, drain its actions, mark the viewer popped out.
6. Tests: unit tests in popout.rs for the shared state and action filtering; a new integration test `crates/sub-ui/tests/viewer_popout.rs` on the shared harness with a snapshot of the pop-out viewer content and an interaction test clicking the menu toggle; headless key test for the shortcut queue. Verify fmt, clippy pedantic and cargo test -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented as a new `sub-ui` module, `popout`:

- `PopoutViewer` owns whether the viewer is out; `PopoutShared` (Arc, behind mutexes and atomics — nothing here is on the audio path) is what the deferred viewport's Send+Sync closure sees. The main pass publishes a `ViewerFrame` (an `egui::TextureId` plus the canvas size) into it once a frame; the closure paints that very id. No second composite, no readback, no copy — `viewer::paint_picture` was lifted out of `ViewerPanel` so both windows draw the picture with the same code.
- `PopoutViewer::show` calls `ctx.show_viewport_deferred` with the stable id `popout_viewport_id()`, so eframe gives it a real OS window the window manager can move to another display (TASK-68 puts it fullscreen on a chosen monitor).
- Keyboard: the closure polls the playback half of the editor's `ShortcutMap` (`playback_shortcuts`, filtered by `is_playback_action`) in the pop-out viewport's own input, queues what fired and asks the root viewport to repaint. `SubordinateApp::run_popout` drains that queue and applies it to `ViewerState` and `PlaybackScheduler`, so the playhead is still only ever moved by the main pass. Non-playback chords are left for the editor window rather than swallowed.
- Closing: the closure watches `viewport().close_requested()` and sets the shared flag; `poll_close` clears it on the next main pass, and `ViewerPanel::popped_out` goes false, so the docked panel takes the picture straight back. While it is out the docked tab keeps its transport row and scrub bar and says `POPPED_OUT_LABEL` where the picture was, rather than showing a stale frame.
- Wiring: a Viewer item in the View menu (`popout_menu_ui`, one item both ways), and `run_popout` between the composite and the dock in `SubordinateApp::ui`.

Notes on the environment: this machine renders through a software wgpu adapter, so the snapshot was recorded and compared here; there is no second monitor and no window is created by any test. The deferred viewport as a real OS window on a second display stays TASK-118's verification, as AC #4 says.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the pop-out viewer: a View-menu item pops the preview into an egui deferred viewport (its own OS window) that paints the same compositor texture the docked viewer paints — the picture crosses as an egui::TextureId, so there is no extra render pass — and closing that window returns the picture to the dock on the next frame. The pop-out answers the playback half of the keymap (J/K/L, space, frame steps, Home/End) by queueing actions for the main pass, which applies them to the viewer state and the playback scheduler. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test -p sub-ui (all green), including the new crates/sub-ui/tests/viewer_popout.rs on the shared harness: a wgpu snapshot of the pop-out window's picture (tests/snapshots/viewer_popout_window.png), an interaction test clicking the menu item out and back, one closing the pop-out's window and asserting the docked panel takes the same texture back, and one pressing transport keys in the pop-out and asserting the playhead moved and playback started. The deferred viewport on real second-display hardware remains TASK-118's verification.
<!-- SECTION:FINAL_SUMMARY:END -->
