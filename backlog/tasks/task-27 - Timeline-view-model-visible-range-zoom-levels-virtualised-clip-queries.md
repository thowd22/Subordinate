---
id: TASK-27
title: 'Timeline view model: visible range, zoom levels, virtualised clip queries'
status: Done
assignee:
  - '@opus-task-27'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 13:52'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-12
references:
  - docs/PLAN.md
priority: high
ordinal: 48000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A timeline with hundreds of clips must only lay out what is visible (§5.7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 View model maps pixels to RationalTime and back for a zoom range from whole sequence to single frame
- [x] #2 Query returns only clips intersecting the visible range per track
- [x] #3 Unit tests cover zoom math at extreme levels
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub-time and sub-model deps to sub-ui; add a crate-level codes module (ui.invalid_zoom).
2. New module crates/sub-ui/src/timeline.rs holding the view model, independent of egui so it is unit-testable headlessly.
3. ZoomLevel: an exact Rational of pixels per frame, clamped between MIN (1/65536 px/frame, whole multi-hour sequence) and MAX (1024 px/frame, one frame far wider than any viewport); validated new() -> SubResult, plus clamped(), fit(duration,width) and zoomed(steps) doubling/halving. All fraction math in integers (i128), never floats.
4. TimelineView: sequence rate, zoom, viewport width in pixels and a scroll offset in pixels from time zero. Maps pixels to RationalTime (floor: the frame under the pixel) and RationalTime back to pixels (exact rational, converted to f32 only at the painting boundary). zoom_to keeps the time under an anchor pixel fixed; visible_range() returns the half-open TimeRange covering every frame the viewport touches (start floored, end ceiled).
5. TrackLayout: build once per track (O(n)) from Track::clip_placements, storing sorted non-overlapping ClipPlacements; visible(range) binary-searches (partition_point) and returns a slice, so a query costs O(log n + visible) rather than O(n).
6. Tests: round-trip pixel/time at 1/65536 and 1024 px/frame, fit-whole-sequence, single-frame zoom, anchored zoom, scroll clamping, visible-range boundary (clips touching each edge included, others excluded), empty viewport, and a 1000-clip virtualisation test asserting only the visible slice is returned.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-ui/src/timeline.rs, an egui-free view model, plus a crate codes module (ui.invalid_zoom) and sub-model/sub-time dependencies for sub-ui.

ZoomLevel is an exact Rational of pixels per frame on a ladder from 1/65536 (a 45-minute 24 fps sequence in a pixel column) to 1024 (one frame wider than any viewport). new() validates and returns SubError ui.invalid_zoom; clamped(), fit(frames, width) and zoomed(steps) pin to the ladder instead. All fraction work is u128/i128 integer arithmetic.

TimelineView holds the sequence rate, zoom, viewport width and a scroll offset in pixels (pixels, not time, so scrolling stays smooth when a pixel is a fraction of a frame; clamped at the origin). time_at_pixel floors to the frame the pixel column falls in, absolute_pixel_of goes back, visible_range floors the start and ceils the end so partly visible frames count, and fit()/zoom_to_frame() reach the two ends of the range. zoom_to keeps the instant under an anchor pixel fixed, to the precision the new zoom can express. pixel_of is the single float in the module and only at the painting boundary: the position is computed exactly in integers and divided once.

TrackLayout indexes a track's clips once (O(n), from Track::placements, keeping each clip's item index so the panel reaches the clip without searching) and answers visible(range) with a partition_point binary search returning a contiguous slice, so a query costs O(log n + visible) however long the track. Intersection is half-open at both ends and an empty range selects nothing.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui: 27 unit tests and 1 doc test pass, 15 of them new here (zoom ladder validation and clamping, doubling/halving to the stops, pixel/time round trips at 1 px/frame, at ZoomLevel::MAX resolving a single frame and at ZoomLevel::MIN holding a whole sequence, negative pixels, fit, anchored zoom, scroll clamping, subpixel painting, cross-rate times, empty viewport, gap-aware layout, edge-inclusive visible queries, and a 1000-clip track where a 1000-pixel viewport returns exactly the 21 clips it touches).

Environment note: this machine has no system GStreamer and the scratchpad env script it had been using was gone, so sub-ui (which depends on sub-media) could not build at first. Rebuilt the environment against the Flatpak freedesktop SDK 22.08 runtime found on /mnt/d (gstreamer 1.20) via a sysroot symlink, which restored the documented scratchpad env-gst.sh; the checks above all ran for real.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the timeline view model in crates/sub-ui/src/timeline.rs: ZoomLevel, an exact rational pixels-per-frame ladder from a whole multi-hour sequence (1/65536 px/frame) to a single frame filling the viewport (1024 px/frame), validated with the new ui.invalid_zoom error code; TimelineView, which maps pixels to RationalTime and back in pure integer arithmetic (a float appears only in pixel_of, at the painting boundary) and reports the half-open visible range; and TrackLayout, which indexes a track's clips once and answers per-track visibility queries with a binary search returning a contiguous slice, so painting cost follows what is on screen rather than track length. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-ui (27 unit tests plus a doc test, 15 new, covering both extremes of the zoom range, anchored zoom, scroll clamping and a 1000-clip track that returns only the 21 visible clips).
<!-- SECTION:FINAL_SUMMARY:END -->
