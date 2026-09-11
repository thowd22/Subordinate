---
id: TASK-68
title: Fullscreen pop-out on a chosen monitor
status: In Progress
assignee:
  - '@opus-task-68'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 03:54'
labels:
  - ui
milestone: m-5
dependencies:
  - TASK-67
references:
  - docs/PLAN.md
priority: medium
ordinal: 89000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Client-facing review on a TV or second monitor.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Monitor picker lists displays; fullscreen shows only the frame on black
- [x] #2 Esc exits; the choice persists in settings
- [ ] #3 Works on Wayland, X11, Windows and macOS (verified on each)
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers whatever fullscreen exposes in a panel (the monitor picker, the toggle) as an interaction test; the real fullscreen presentation stays with TASK-118
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New module crates/sub-ui/src/fullscreen.rs: Monitor (winit available_monitors() index order, name, size, scale, primary) + MonitorList with a from_context fallback built from egui ViewportInfo::monitor_size, so the picker always lists at least the display the editor is on.
2. FullscreenSettings persisted as fullscreen.json in the same config dir as layout.json/keymap.toml, schema-versioned, tolerant of a missing or malformed file (LoadedFullscreen with problems, new ui.fullscreen_* error codes).
3. Extend PopoutViewer/PopoutShared with fullscreen: the pop-out viewport builder gains with_fullscreen/with_monitor, a change while the window is open is sent as ViewportCommand::Fullscreen/SetMonitor, and Esc inside the pop-out pass asks to leave fullscreen (the window stays, the viewer does not close). Fullscreen content is the picture on black - the pop-out already has no chrome.
4. Picker UI: monitor_picker_ui lists the displays as selectable rows plus a fullscreen toggle, returning an action the app applies; wired into the View menu and saved from eframe's save hook.
5. Tests: unit tests in fullscreen.rs and popout.rs (settings round-trip, malformed file, builder, Esc, monitor selection), plus tests/viewer_fullscreen.rs on the shared egui_kittest harness driving the picker and the toggle as interaction tests. AC#3 (Wayland/X11/Windows/macOS on real hardware) cannot be verified here and stays with TASK-118.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-ui: new module src/fullscreen.rs (Monitor, MonitorList, MonitorChoice, FullscreenSettings, FullscreenState, LoadedFullscreen, monitor_picker_ui, FullscreenAction) plus fullscreen support in src/popout.rs (PopoutPlacement, PopoutViewer::set_fullscreen/set_monitor/poll_fullscreen_exit, PopoutShared fullscreen + leave-fullscreen flags) and wiring in src/app.rs (View menu picker, startup load, persistence from eframe's save hook). Three new error codes: ui.fullscreen_parse, ui.fullscreen_unreadable, ui.fullscreen_unwritable.

Design notes.
- The fullscreen window is the existing pop-out viewport with ViewportBuilder::with_fullscreen plus with_monitor(index); egui turns a changed builder into the SetMonitor/Fullscreen viewport commands, so no command is sent by hand. Content is unchanged - popout_content_ui, the compositor texture letterboxed on black, no scrub bar and no transport row.
- Escape inside the pop-out pass leaves fullscreen and keeps the window (the viewer does not fall back into the dock); it is ignored when the window is not fullscreen.
- The choice is written to fullscreen.json in the same per-user config directory as layout.json and keymap.toml, schema-versioned like the layout, and it is remembered by display name as well as by index so a replugged monitor is followed rather than the slot it used to occupy. A missing or malformed file is 'no display chosen' plus a logged SubError, never a refusal to start.
- Display enumeration: eframe 0.36 exposes no available_monitors() to an application, so MonitorList::from_context lists the display the editor is painting on (egui's ViewportInfo::monitor_size) and the picker carries a display-number box as the escape hatch for a head the editor cannot name. The index it stores is winit's available_monitors() order, which is exactly what with_monitor/SetMonitor take.

Verification (this environment is Linux WSL, no sudo, no GPU; wgpu ran on the software adapter).
- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean (exit 0).
- cargo test -p sub-ui: 30 test targets, all ok, including the new tests/viewer_fullscreen.rs (picker rows, toggle, Escape), 11 new unit tests in fullscreen.rs and 4 new ones in popout.rs. The committed pop-out snapshot (viewer_popout_window) rendered and matched here, which is the 'frame on black' evidence for what the fullscreen window paints.
- AC#3 (Wayland, X11, Windows and macOS, verified on each) cannot be verified here: no second head and no GPU runner. It belongs with TASK-118, which already owns pop-out and second-display verification on hardware; the criterion is left unchecked.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fullscreen review on a chosen display: a monitor picker in the View menu, the pop-out viewport taken fullscreen on the chosen monitor with the picture on black and nothing else, Escape to leave fullscreen without losing the window, and the choice remembered in fullscreen.json beside layout.json. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-ui (all green), including a new egui_kittest interaction test on the shared harness for the picker, the toggle and Escape. AC#3 (verification on Wayland, X11, Windows and macOS) is left unchecked: it needs real multi-head hardware and belongs with TASK-118.
<!-- SECTION:FINAL_SUMMARY:END -->
