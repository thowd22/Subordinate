---
id: TASK-29
title: 'Playhead, snapping and click-to-seek on the timeline'
status: Done
assignee:
  - '@opus-task-29'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 10:20'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-28
  - TASK-23
references:
  - docs/PLAN.md
priority: high
ordinal: 50000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Precise placement relies on snapping to clip edges, markers and the playhead.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Clicking the ruler seeks; dragging scrubs with viewer updates
- [x] #2 Snap targets: clip edges, markers, playhead, sequence start; toggle with S
- [x] #3 Snap threshold is in pixels and independent of zoom
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers this: an interaction test that clicks the ruler and asserts the playhead time, plus a snapshot of the panel with the playhead drawn
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a snapping module in sub-ui (snapping.rs): SnapKind (sequence start, clip edge, marker, playhead), SnapCandidate, SnapSettings { enabled, threshold_px } with a pixel threshold, candidate collection from the visible range of the synced TrackLayouts plus sequence markers and the playhead, and a nearest-candidate search whose distance is compared exactly in integers (frames * pixels_per_frame), so the threshold is in pixels and zoom-independent.
2. Give TimelinePanel a playhead RationalTime, snap settings and scrub state; paint the playhead as a line through the ruler and lanes with a head in the ruler.
3. Click-to-seek and scrub: a press in the ruler starts a scrub, drag continues it, the requested time is snapped when snapping is on, and the panel reports it as TimelineResponse::seek plus the candidate it snapped to. The panel stays read-only; the playhead is not project state, so it is not a Command.
4. Wire it up in app.rs: feed the viewer playhead into the panel, apply the reported seek to the viewer state, and bind Action::ToggleSnapping (S) to the panel's snap toggle.
5. Tests: unit tests for candidate collection, nearest-within-threshold and zoom independence; an egui_kittest interaction test on the shared harness that clicks the ruler and asserts the playhead time, and a snapshot of the panel with the playhead drawn.
6. cargo fmt, clippy -D warnings, cargo test -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in three pieces.

New `crates/sub-ui/src/snapping.rs`: `SnapKind` (Playhead, Marker, ClipEdge, SequenceStart, declared in tie-break priority order), `SnapCandidate`, `SnapSettings { enabled, threshold_px }` with `DEFAULT_THRESHOLD_PX = 8`, `collect_candidates` (visible-range only, through the panel's existing `TrackLayout` binary-search index, so a scrub costs O(log n) per track) and `snap`/`snapped_time`. Distances are compared with no floats and no seconds: a frame difference is cross-multiplied against the zoom's pixels-per-frame fraction in i128, so the threshold is exactly a screen distance and identical at every zoom.

`TimelinePanel` gains a `playhead: RationalTime`, the snap settings, a scrub flag and a reused candidate buffer. `handle_scrub` turns a press in the ruler into a scrub and every frame the button stays down continues it, so a click and a drag are one gesture; the time is snapped, the panel's own playhead moves before it paints, and the frame reports `TimelineResponse::seek` plus the `SnapCandidate` it landed on. `paint_playhead` draws the line down the ruler and lanes with a triangular head, clipped so it never crosses the header column, plus a yellow flag on the edge a live scrub is snapping to. The playhead is view state, not project state, so nothing here is a Command; `sync` rescales it when the sequence timebase changes.

`app.rs` closes the loop: the viewer's playhead is handed to the panel each frame, `response.seek` is applied with `ViewerState::seek_to` (which is what asks for a fresh composite), and `Action::ToggleSnapping` (S, already in the default keymap) calls `TimelinePanel::toggle_snapping`.

Snapping targets are sequence markers; clip-anchored markers are in source time and are left to TASK-40, which brings markers to the ruler. The playhead is offered as a target to clip drags (TASK-30) but deliberately excluded from a ruler scrub, since it cannot snap to itself.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui green (13 suites, including the 9 new tests in tests/timeline_playhead.rs and 11 new unit tests in snapping.rs). The snapshot `timeline_playhead.png` was recorded here on a real wgpu adapter, and `harness_timeline_panel.png" was re-recorded because the playhead is now drawn at time zero in it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The timeline now carries a playhead: clicking the ruler seeks and holding the button scrubs, with the viewer following through ViewerState::seek_to. A new sub-ui snapping module pulls the requested time onto clip edges, sequence markers, the playhead and sequence start; the threshold is a pixel distance compared in exact integer arithmetic, so it is the same reach on screen at every zoom, and S toggles it through the existing Action::ToggleSnapping binding. Verified by nine egui_kittest tests on the shared harness (click-to-seek asserting the panel and viewer playhead, a scrub drag, snap on/off, zoom-independence of the threshold, and a committed snapshot of the panel with the playhead drawn) plus eleven unit tests in snapping.rs; cargo fmt --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-ui all pass.
<!-- SECTION:FINAL_SUMMARY:END -->
