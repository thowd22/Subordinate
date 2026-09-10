---
id: TASK-38
title: 'Crossfade transition: model, command and render'
status: Done
assignee:
  - '@opus-task-38'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-10 19:24'
labels:
  - ui
  - render
milestone: m-2
dependencies:
  - TASK-21
  - TASK-31
references:
  - docs/PLAN.md
priority: medium
ordinal: 59000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The only MVP transition (§5.3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 AddTransition/RemoveTransition commands place a crossfade between adjacent clips with a duration clamped to available handles
- [x] #2 Compositor blends the two clips linearly across the transition
- [x] #3 Timeline draws the transition region and allows dragging its duration
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the crossfade's timeline drawing: a committed snapshot of a transition on the timeline, and an interaction test for however it is created or adjusted from the UI
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-model: add Transition offset accessors and a helper for the cut a transition sits at.
2. sub-edit: new commands module transition.rs with AddTransition (transition.add) and RemoveTransition (transition.remove), addressed by the incoming clip at the cut, duration split symmetrically and clamped to the available handles (neighbour durations and probed source handle); undo through RestoreTrackItems like the other track edits; register in builtin registry.
3. sub-render: extend resolve_layers_at so a track under a crossfade yields two layers - the outgoing clip reading into its tail handle and the incoming clip reading into its head handle - with an exact linear blend weight carried on ResolvedClip and multiplied into the shader opacity by the compositor.
4. sub-ui: index transitions in TrackLayout, paint the transition region on the timeline, and add a drag gesture on its edges that plans an AddTransition with the new duration (crates/sub-ui/src/transition.rs, mirroring trim.rs plan/apply).
5. Tests: unit tests in sub-model/sub-edit/sub-render, a sub-ui egui_kittest interaction test for the drag and a committed snapshot of a transition on the timeline (snapshot needs a wgpu adapter; recorded only if one is available here).
6. Verify with cargo fmt, clippy pedantic and the touched crates' tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Model: Transition already existed in sub-model; added in_offset/out_offset accessors.

Commands (crates/sub-edit/src/commands/transition.rs): transition.add and transition.remove address a cut by naming the incoming clip, so a crossfade sits in the item list immediately before it and consumes no track time. The duration is split about the cut (the odd frame going to the half before it) and each half is clamped: the in offset to the outgoing clip's duration and the incoming clip's head handle, the out offset to the incoming clip's duration and the outgoing clip's probed tail handle. An unprobed source is bounded only by the neighbours. New stable codes edit.invalid_transition (reason detail: not_positive, no_cut, no_handle) and edit.transition_not_found. Both commands undo through RestoreTrackItems, so re-adding over an existing crossfade undoes to the exact previous offsets. Applying transition.add over an existing crossfade replaces it, which is what a duration drag commits. docs/schema/command-api.json regenerated.

Render (crates/sub-render/src/graph.rs): resolve_layers_at now yields two layers for a track under a crossfade - the outgoing clip reading into its tail handle, then the incoming clip over it carrying a new ResolvedClip::blend weight computed as an exact integer ratio at the sequence timebase and rounded once into Opacity. The compositor multiplies blend into the clip's own opacity, so premultiplied over-blending gives (1-w)*outgoing + w*incoming.

UI: TrackLayout indexes transitions as the span they blend across; the panel paints that region (wash, outline and the dissolve X) and a press anywhere in it starts a duration drag - the half pressed decides which edge moves, and both edges move symmetrically about the cut, so the cut never shifts. crates/sub-ui/src/transition.rs plans the drag into one AddTransition (previewed through fit_crossfade so the painted region and the committed command agree) and applies it as one undo entry; refusals carry ui.transition_refused. The four fixture-based timeline snapshots were re-recorded because the committed sample project already holds a crossfade that is now drawn.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -D warnings clean; cargo test --workspace green (60 suites); new tests: 13 in sub-edit commands::transition, 6 in sub-render graph, 1 GPU end-to-end in sub-render tests/compositor (a_crossfade_dissolves_one_clip_into_the_other reads back the half-dissolve pixel), 7 in sub-ui transition, 8 in crates/sub-ui/tests/timeline_transition.rs including the committed snapshot timeline_crossfade.png (this machine does have a software wgpu adapter, so the snapshot really rendered).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Crossfades end to end: transition.add and transition.remove place and remove a blend at a cut with its duration split about the cut and clamped to the neighbours' durations and the probed source handles, undoing through RestoreTrackItems; the compositor resolves two layers under a blend and dissolves them linearly through an exact RationalTime-derived weight; and the timeline draws the blend region and drags its duration as one undoable AddTransition. Verified with cargo fmt --check, clippy pedantic -D warnings and a green cargo test --workspace, including a GPU read-back test of the half-dissolve pixel and an egui_kittest interaction test plus committed snapshot for the timeline region.
<!-- SECTION:FINAL_SUMMARY:END -->
