---
id: TASK-21
title: 'Compositor frame graph v0: single track with opacity and transform'
status: Done
assignee:
  - '@opus-task-21'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 13:56'
labels:
  - render
milestone: m-1
dependencies:
  - TASK-20
  - TASK-12
references:
  - docs/PLAN.md
priority: high
ordinal: 42000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Establishes the render-graph structure that later multi-track and effects work extends (§5.3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 render(sequence, time) resolves the clip under the playhead, samples its frame and draws it with opacity and position/scale/rotation into a target texture
- [x] #2 Output resolution follows sequence settings; letterboxing for mismatched aspect is correct
- [x] #3 Renders to an offscreen texture usable both by egui and by readback
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub-model and sub-time as dependencies of sub-render (no cycle: sub-model depends only on sub-core/sub-time).
2. New module sub-render/src/graph.rs holding the v0 frame graph:
   - resolve_clip_at(sequence, time): walk video tracks top-down, skip muted ones, use Track::clip_placements at the sequence frame rate to find the clip whose timeline range contains the playhead; map playhead to source time exactly in RationalTime (source_range.start + offset). Returns ResolvedClip { track, clip, timeline_range, source_time }.
   - LetterboxFit: contain-fit of a source picture inside the sequence canvas, centred, so a mismatched aspect gets correct black bars.
   - QuadTransform: pure CPU maths turning the letterbox rect plus a clip Transform (scale, then clockwise rotation, then translation about the clip centre, y down) into a 2x2 canvas-pixel basis plus an offset, and into the uniform bytes the shader reads. Corner helper for tests.
   - FrameSource trait: the compositor asks the caller for the RGB texture view of a resolved clip (SourceFrame { view, width, height }); decoding stays in sub-media, so sub-render keeps no GStreamer dependency.
   - Compositor: owns the shared RenderContext and one offscreen Rgba8UnormSrgb target with RENDER_ATTACHMENT | TEXTURE_BINDING | COPY_SRC so egui can sample it and export can read it back. render(sequence, time, source) resizes the target to the sequence resolution if it changed, clears to opaque black (the letterbox), then draws one alpha-blended quad with the clip opacity. read_rgba() does the padded copy-texture-to-buffer readback.
3. Export the new items from lib.rs; keep RenderError codes untouched (no new failure modes).
4. Tests: unit tests for clip resolution (gaps, muted tracks, boundaries, out of range, source-time mapping) and for letterbox/quad geometry, all float-free on the model side; a GPU integration test (tests/compositor.rs, skipped when no adapter) rendering a solid source into a wider canvas and checking pillarbox bars, centred picture, opacity blend, translation, 90-degree rotation and readback size.
5. Verify with cargo fmt --check, clippy pedantic -D warnings and cargo test -p sub-render.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in a new module crates/sub-render/src/graph.rs, re-exported from lib.rs. sub-render now depends on sub-model and sub-time (no cycle; sub-model depends only on sub-core and sub-time), and still has no GStreamer dependency.

Design notes:
- resolve_clip_at(sequence, time) is pure model arithmetic and float-free. It rescales the playhead to the sequence timebase, walks the video tracks in reverse (top-down compositing order), skips muted tracks, and maps the playhead into source time as source_range.start() + (playhead - timeline_range.start()), all in RationalTime. That is already the multi-track walk of PLAN §5.3 stopped after the first hit, so TASK-39 extends it rather than replacing it.
- LetterboxFit is a contain-fit: the whole picture is visible, centred, with black bars on whichever pair of sides the aspect mismatch demands. The clear-to-black of the pass IS the letterbox; nothing is cropped.
- QuadTransform turns the fit plus a clip Transform into a 2x2 canvas-pixel basis and an offset, applying scale, then clockwise rotation, then translation about the clip centre, with y down as Point2 documents. It also packs the shader's 48-byte uniform block. Floats appear only here, at the edge, converted once from Fixed6 via as_f32.
- FrameSource is the seam: the compositor asks the caller for the already-converted RGB texture view of a resolved clip (SourceFrame), so decoding and NV12 conversion stay in sub-media and Nv12Converter. Any FnMut(&ResolvedClip) -> Option<SourceFrame> is a source. Returning None composites the bare black canvas rather than a stale frame.
- Compositor::render resizes the target to sequence.settings.resolution first, so the canvas follows the sequence with no separate call, then clears to opaque black and draws one alpha-blended quad. Blending happens in linear light because the target is Rgba8UnormSrgb, which is why half opacity over black reads back as ~188, not 128.
- The target carries RENDER_ATTACHMENT | TEXTURE_BINDING | COPY_SRC. read_rgba() does the padded copy-texture-to-buffer readback and strips the row padding for the export path (TASK-58).

No new RenderError variants: the graph has no failure mode of its own. Errors keep the existing stable codes untouched.

Verification (WSL, software Vulkan adapter via lavapipe):
- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean (GStreamer env sourced so sub-media and its dependents build).
- cargo test -p sub-render: 28 unit tests pass, including 13 new ones in graph::tests covering clip resolution (clip interiors, the end-exclusive boundary, gaps, past the end, negative time, an empty sequence, a playhead handed in at 96 kHz, top-track and muted-track selection, source-time mapping into a clip whose source starts at frame 100) and the letterbox/quad geometry (matching aspect, pillarbox, letterbox, zero-sized source, identity placement, position with y down, scale and mirror, 90-degree clockwise rotation, uniform packing).
- cargo test -p sub-render --test compositor: 7 new GPU tests pass. They composite a four-quadrant source and read the canvas back, proving orientation (AC 1), pillarbox and letterbox bar positions and that the canvas follows a changed sequence resolution (AC 2), opacity blending and the transform, a gap compositing to black, and that the same target both binds as a sampled texture and copies back to the CPU (AC 3). Like the other GPU tests in this crate they report and pass where the machine has no wgpu adapter.

AC 3 caveat: the test proves the target satisfies the egui path's requirement by binding it as a sampled texture on the shared device (wgpu rejects that without TEXTURE_BINDING); registering it with egui's renderer is TASK-22's viewer panel, which does not exist yet.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the v0 compositor frame graph in crates/sub-render/src/graph.rs: resolve_clip_at finds the clip under the playhead top-down across video tracks and maps the playhead into source time entirely in RationalTime; LetterboxFit and QuadTransform contain-fit the source in the sequence canvas and apply the clip transform (scale, then clockwise rotation, then translation about the clip centre); and Compositor renders that as one alpha-blended quad, scaled by the clip opacity, into an offscreen Rgba8UnormSrgb target sized from the sequence settings and usable both as a sampled texture and via read_rgba() readback. Pictures come from a FrameSource trait so decoding stays in sub-media. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, 28 unit tests and 7 new GPU integration tests in tests/compositor.rs that read the composited canvas back and assert on bar positions, quadrant orientation, opacity blending and the transform.
<!-- SECTION:FINAL_SUMMARY:END -->
