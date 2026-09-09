---
id: TASK-28
title: 'Timeline panel: ruler, track lanes, clip rectangles, scroll and zoom'
status: Done
assignee:
  - '@opus-task-28'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 15:10'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-27
references:
  - docs/PLAN.md
priority: high
ordinal: 49000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The central editing surface, painted directly with egui's Painter for performance.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Ruler shows timecode ticks appropriate to zoom; tracks render top-down with headers
- [x] #2 Clips render with name, colour by media type and trimmed edge indicators
- [x] #3 Ctrl+wheel zooms around cursor, wheel scrolls; 500 clips render at 60 fps (measured)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-ui/src/timeline_panel.rs: a TimelinePanel that paints the timeline with egui's Painter on top of the TASK-27 TimelineView/TrackLayout view model.
2. Ruler: an exact integer tick ladder (frames, then nominal-second, minute and hour multiples) chosen by zoom; major ticks labelled with SMPTE Timecode, minor ticks unlabelled. No floats in the tick maths.
3. Track lanes: tracks painted top-down in sequence order with a fixed-width header column (name, kind, mute/lock state), lanes virtualised through TimelineView::visible_clips.
4. Clips: rounded rectangles coloured by media kind (video/audio/still/offline/unknown), name text drawn only when the rectangle is wide enough, and trimmed-edge bars on the head/tail when the clip does not use the whole source.
5. Input: ctrl+wheel (egui zoom_delta) zooms around the pointer via TimelineView::zoom_to; plain wheel scrolls the lanes vertically and horizontal wheel scrolls time. Add ZoomLevel::scaled so a wheel notch is an exact rational step.
6. Layout cache keyed on an engine revision so TrackLayout indexes are rebuilt per edit, not per frame.
7. Tests: unit tests for the tick ladder, media colouring, trim detection and wheel handling; a headless egui::Context::run test that paints 500 clips and measures the frame time against a 16.6 ms budget.
8. Verify with cargo fmt --check, clippy -D warnings and cargo test -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented crates/sub-ui/src/timeline_panel.rs: TimelinePanel paints the whole editing surface with egui's Painter (no widgets per clip), on top of the TASK-27 TimelineView/TrackLayout view model.

Ruler: tick_interval_frames walks an integer ladder (sub-second steps that divide the rate exactly, then whole timecode seconds, minutes and hours) and picks the finest step at least 96 points wide for labels and 12 points for minor ticks; labels come from sub_time::Timecode (drop-frame when the rate drops frames), falling back to a frame number for rates timecode cannot count. Tick positions are whole frames anchored to sequence zero, so all tick maths is integer.

Lanes: tracks paint top-down in sequence order into a 132-point header column (name, kind, muted/locked) plus a virtualised lane area; visible_tracks skips off-screen lanes vertically and TimelineView::visible_clips skips off-screen clips horizontally.

Clips: rounded rectangles filled by ClipMediaKind (video/audio/still/offline/unknown, one colour each), outlined, with a 3-point bar on any trimmed head or tail edge (TrimmedEdges::of compares the clip source range against the probed media duration; an unprobed source never claims a trimmed tail), and the clip name drawn only when the rectangle is at least 26 points wide.

Input: egui routes ctrl+wheel and pinch to zoom_delta, so a zoom gesture goes through TimelineView::zoom_to anchored at the pointer and the frame under the cursor stays put; the plain wheel scrolls time horizontally and the lanes vertically, clamped at both ends. Added ZoomLevel::scaled so a wheel factor becomes an exact rational step rather than a float multiply.

The panel is read-only: it never mutates the project, so no Command is involved. Layouts are rebuilt by sync(sequence, revision) against the engine revision, not per frame.

Performance (AC3) measured headlessly with egui::Context::run_ui plus tessellation, which is the whole CPU cost of a frame: 500 clips across 4 tracks, best frame 1.23 ms zoomed out with every clip on screen and 0.09 ms zoomed in with clip names painted, against a 16.67 ms 60 fps budget. This machine has no GPU, so the GPU upload/draw of the resulting few thousand flat triangles is not included; the test asserts the CPU frame budget and prints best/median/worst.

Not in scope and deliberately left out: the panel is not yet placed in SubordinateApp, which has no project or engine wired into it; TASK-43 (dockable panel layout) is where panels are mounted. Playhead, selection and dragging are TASK-29 onwards.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui green (35 unit tests, 2 headless paint tests, 1 doc test).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the timeline panel (crates/sub-ui/src/timeline_panel.rs): a Painter-drawn editing surface with a zoom-aware timecode ruler, top-down track lanes with headers, clip rectangles coloured by media kind and marked on trimmed edges, ctrl+wheel zoom around the pointer and wheel scrolling, all built on the TASK-27 view model with integer frame maths throughout. Verified with 15 new unit tests (tick ladder, timecode labels, media colouring, trim detection, wheel handling, index syncing), a headless egui paint test proving virtualisation, and a measured 500-clip frame of 1.23 ms best against the 16.67 ms 60 fps budget; cargo fmt, clippy -D warnings and cargo test -p sub-ui are all clean.
<!-- SECTION:FINAL_SUMMARY:END -->
