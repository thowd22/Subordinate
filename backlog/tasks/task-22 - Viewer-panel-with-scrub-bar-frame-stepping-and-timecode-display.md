---
id: TASK-22
title: 'Viewer panel with scrub bar, frame stepping and timecode display'
status: In Progress
assignee:
  - '@opus-task-22'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 15:09'
labels:
  - ui
milestone: m-1
dependencies:
  - TASK-21
  - TASK-18
references:
  - docs/PLAN.md
priority: high
ordinal: 43000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
First user-visible milestone: open a file and scrub it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Viewer shows the compositor texture scaled to fit with correct aspect
- [x] #2 Scrub bar drag, left/right arrow frame step, Home/End work and display the timecode from sub-time
- [ ] #3 Scrubbing the 4K fixture on a hardware-decode Linux machine sustains above 30 fps (measured and recorded in the task notes)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-ui/src/viewer.rs: an egui-free view model (ViewerState holding rate, duration and playhead as RationalTime, integer pixel-to-frame scrub mapping, frame stepping and start/end jumps, timecode label via sub_time::Timecode with a frame-count fallback) plus ViewerFit aspect-fit maths and a key-to-action map. 2. Add the egui ViewerPanel: black canvas painting the compositor texture at the fitted rect, draggable scrub bar, step and start/end buttons, timecode readout, and Left/Right/Home/End keys handled when no text field has focus. 3. Wire the viewer into SubordinateApp: build a Compositor on the shared RenderContext, render the sequence at the playhead each frame with a null FrameSource, register the compositor output with egui's wgpu Renderer once per output texture, and show the panel. 4. Unit tests for fit maths, scrub mapping, stepping and clamping, timecode formatting and key mapping; run fmt, clippy -D warnings and cargo test -p sub-ui. 5. AC 3 needs a GPU and a system GStreamer, neither of which exists in this environment, so leave it unchecked with a note.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-ui/src/viewer.rs. ViewerState keeps the playhead and the sequence length as frame numbers at the sequence timebase, so scrubbing, stepping and clamping are exact integer moves and no time is ever a float; the only floats are the paint-time position of the playhead marker and ViewerFit, which fits the canvas into the panel area preserving aspect (contains, never crops or stretches). Timecode comes from sub_time::Timecode, drop-frame at NTSC rates, with a 'frame N' fallback at rates timecode cannot label. The playhead is view state, not project state, so it is not a Command and is not undoable; no new error codes were needed. SubordinateApp now owns a Compositor over an empty default sequence, renders it at the playhead only when the playhead moved, registers the compositor output with egui's wgpu Renderer once per canvas size (freeing the old id on a resize) and paints it through ViewerPanel. The FrameSource is still empty: TASK-23 supplies decoded pictures, so today the composite is the black canvas. Verified: cargo fmt --all --check clean, cargo clippy --workspace --all-targets -- -D warnings clean, cargo test -p sub-ui 55 unit + 2 doc tests pass. AC 1 and AC 2 are proven by headless egui tests that paint the panel and inspect the shapes it emitted: the compositor texture is drawn as a mesh whose bounds hold the 16:9 canvas aspect within 0.01 and never exceed the panel; ArrowLeft, ArrowRight, Home and End move the playhead through ViewerPanel::ui; a synthetic press-and-drag on the scrub bar seeks to the expected frame (400px along an 800px bar over 240 frames is frame 120); and the painted text contains the sub-time timecode of the playhead and the duration. The whole app was also run headlessly with SUB_SMOKE_FRAMES=5, which painted five frames through the viewer on llvmpipe with no wgpu validation error.

AC 3 is left unchecked: it needs a hardware-decode Linux machine with a real GPU and a system GStreamer, and this environment has neither (software llvmpipe only, GStreamer extracted into a user prefix, no 4K fixture decode path wired into the viewer yet). The measurement also depends on the playback scheduler and decoder feeding the compositor's FrameSource, which is TASK-23; it should be measured there or on the benchmark harness in TASK-26.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the viewer panel: an egui-free ViewerState holding the playhead and sequence length as exact frame numbers, ViewerFit for aspect-preserving letter/pillarboxing, and a ViewerPanel that paints the compositor texture, a draggable scrub bar, step and start/end buttons and a sub-time timecode readout, with Left/Right/Home/End on the keyboard. SubordinateApp now builds a Compositor on the shared wgpu device, composites the sequence at the playhead when it moves and registers that output with egui once per canvas size. Verified with 55 unit tests plus 2 doctests in sub-ui, including headless egui paints that assert the texture quad keeps the 16:9 canvas aspect, that the transport keys and a synthetic scrub-bar drag move the playhead, and that the timecode is painted; fmt and workspace clippy -D warnings are clean and a SUB_SMOKE_FRAMES=5 run painted the real app on llvmpipe. AC 3 stays unchecked: it needs a GPU with hardware decode and the decoder wiring from TASK-23.
<!-- SECTION:FINAL_SUMMARY:END -->
