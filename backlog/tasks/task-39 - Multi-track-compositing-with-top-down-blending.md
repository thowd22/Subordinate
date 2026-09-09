---
id: TASK-39
title: Multi-track compositing with top-down blending
status: Done
assignee:
  - '@opus-task-39'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 16:50'
labels:
  - render
milestone: m-2
dependencies:
  - TASK-21
  - TASK-34
references:
  - docs/PLAN.md
priority: high
ordinal: 60000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Stacked video tracks must composite in order with per-clip opacity (§5.3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Compositor iterates video tracks top-down, skipping muted tracks, blending with premultiplied alpha
- [x] #2 Gaps are transparent; the bottom is black
- [x] #3 A golden test with two overlapping colour-bar clips at 50% opacity matches reference
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Replace the single-hit walk in sub-render/src/graph.rs with resolve_layers_at(): a bottom-to-top iterator over unmuted video tracks yielding one ResolvedClip per track; keep resolve_clip_at() as its topmost element so existing callers are unchanged.
2. Make the composite pass draw every layer: one uniform buffer with an alignment-aware per-layer stride (grown on demand), one bind group per layer, draws issued bottom-first inside a single render pass cleared to opaque black.
3. Switch the fragment shader and blend state to premultiplied alpha (fs returns rgb*a, BlendState::PREMULTIPLIED_ALPHA_BLENDING) so stacked layers accumulate correctly.
4. Widen FrameSummary into a list of LayerSummary (track, clip, source time, opacity, placement) with top()/drawn helpers; update the UI and existing tests.
5. Tests: unit tests for the layer walk (order, muted skipped, audio ignored, gaps); GPU tests in tests/compositor.rs for top-down order, muted tracks, a gap on the upper track letting the lower show through and the bottom being black; a new tests/composite_golden.rs golden with two overlapping colour-bar clips at 50% opacity compared against sRGB/linear-computed reference values.
6. Verify with cargo fmt --check, clippy --workspace --all-targets -D warnings and cargo test for sub-render and sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

- crates/sub-render/src/graph.rs: resolve_layers_at() replaces the single-hit walk. It yields one ResolvedClip per video track in composite order (bottom track first), skipping muted tracks, audio tracks and tracks showing a gap under the playhead; all arithmetic stays in RationalTime. resolve_clip_at() is now its topmost element, so existing callers are unchanged.
- Compositor::render() draws every resolved layer in one render pass over the opaque-black clear, bottom track first. Each layer gets its own slice of one uniform buffer, spaced by the device's min_uniform_buffer_offset_alignment; the buffer only grows, so a steady track count stops reallocating after the first frame.
- The fragment shader now returns premultiplied colour (rgb*a, a) and the pipeline blends with BlendState::PREMULTIPLIED_ALPHA_BLENDING, so stacked layers accumulate coverage correctly instead of only compositing over an opaque background.
- FrameSummary became a bottom-up Vec<LayerSummary> (track, clip, source time, opacity, placement) with top(), drawn() and is_blank(); LayerSummary and resolve_layers_at are re-exported from the crate root. sub-ui needed no change.

Verification (all run in this worktree)

- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean (with the local GStreamer prefix on PKG_CONFIG_PATH so sub-media builds).
- cargo test -p sub-render: 49 tests pass on the software Vulkan adapter (lavapipe). New unit tests cover the layer walk order, muted tracks, audio tracks and gaps; new GPU tests in tests/compositor.rs cover an opaque top track covering the one below, a muted top track uncovering it, a gap above being transparent over a letterboxed lower layer whose bars stay black, an empty stack being black, and a half-opacity upper track blending over the lower one.
- New tests/composite_golden.rs is the AC #3 golden: nine colour bars on a lower track and the same bars rotated by four on the track above, both clips at 50% opacity, compared bar by bar against an sRGB->linear->over->sRGB reference computed on the CPU (+-3 codes). Swapping the two layers in the expected value was checked to fail the test, so it is order-sensitive rather than a smoke test. It also asserts the opaque and muted variants of the same stack.

cargo test -p sub-ui also passes (14 tests) after the FrameSummary change; the app only calls Compositor::render and ignores the summary, so no UI change was needed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The compositor now walks every video track instead of only the frontmost one. resolve_layers_at() yields one ResolvedClip per unmuted video track in composite order (bottom first), skipping audio tracks and gaps, all in RationalTime; Compositor::render draws each layer over the opaque-black clear with its own opacity and transform from its own aligned slice of one uniform buffer, and the shader plus BlendState::PREMULTIPLIED_ALPHA_BLENDING blend them with premultiplied alpha. FrameSummary is now a bottom-up list of LayerSummary. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, cargo test -p sub-render (49 tests, software Vulkan adapter) and cargo test -p sub-ui (14 tests); the new tests/composite_golden.rs compares two 50%-opacity colour-bar clips against a CPU-computed linear-light reference and was mutation-checked to fail when the layer order is swapped.
<!-- SECTION:FINAL_SUMMARY:END -->
