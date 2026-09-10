---
id: TASK-87
title: Shader effect execution from plugin declarations in the compositor
status: Done
assignee:
  - '@opus-task-87'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 05:50'
labels:
  - render
  - plugins
milestone: m-6
dependencies:
  - TASK-76
  - TASK-84
  - TASK-39
references:
  - docs/PLAN.md
priority: high
ordinal: 108000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Connects the effect world to sub-render (§5.3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Compositor compiles the plugin's WGSL with a validated uniform layout from ParamDesc and caches by hash
- [x] #2 Effects are applied per clip in order; a failing shader compile disables that effect with a visible error
- [x] #3 Golden test: reference tint effect changes pixel values as expected
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub-render/src/effect.rs: EffectDesc/EffectParam/ParamKind/ParamValue mirroring the WIT effect-types records, with validation (unique lowercase ids, ranges, defaults in range, non-empty shader/entry).
2. Build a validated uniform layout from the declared params: one 16-byte slot per param via @align(16) @size(16), generated WGSL struct plus a byte packer for ParamValue.
3. Compile prelude+plugin WGSL into a render pipeline behind an EffectCache keyed by a blake3 hash of shader, entry and layout; a validation failure is cached as a disabled effect carrying the compiler message.
4. Run effects per clip in declaration order in the compositor with ping-pong offscreen textures before transform/opacity/blend; report failures in FrameSummary so the UI can show them.
5. sub-plugin: optional 'render' feature converting WitEffectDesc/WitParamDesc into sub_render::EffectDesc.
6. Tests: layout/validation/cache units, a golden tint test asserting expected pixel change, order test, and a broken-shader test proving the effect is skipped with an error.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
sub-render gains an `effect` module: EffectDesc/EffectParam/ParamKind/ParamValue mirror the WIT effect-types records without dragging WIT or wasmtime into the GPU crate. A declaration is validated on construction (lowercase unique ids, finite ranges holding their defaults, non-empty shader, entry point that is a WGSL identifier and is not the host's own vs_effect) and derives a uniform layout: one 16-byte slot per parameter with @align(16) @size(16), so the offset of parameter n is 16*n on every backend and the WGSL struct and the byte packer cannot disagree. EffectCache compiles prelude+plugin WGSL behind a blake3 EffectKey over the module source and entry point, and caches failures as well as pipelines so a bad shader asks the driver once. Shader errors are captured with a wgpu validation error scope, so a plugin's broken WGSL never reaches the uncaptured-error handler.

Compositor::render_with_effects runs each clip's chain in declaration order on the clip's own picture, before the letterbox fit, transform and opacity, ping-ponging between a per-layer pair of offscreen textures (per layer, not shared, because the composite pass samples them all after the chains have run). A failing effect is skipped, the rest of the chain still runs, the clip still draws, and the failure is reported in FrameSummary::effect_failures with the compiler's message and the render.effect_compile_failed code. Compositor::render is that call with NoEffects, so existing callers are unchanged apart from the two new summary fields.

sub-plugin gains an optional `render` feature (optional sub-render dependency) carrying the WIT->sub-render conversion: effect_desc, param_desc, param_kind, param_value and effect_instance. It is off by default so the headless MCP host builds no wgpu; the crate's own tests turn it on. New RenderError variants carry stable codes render.invalid_effect_param, render.invalid_effect_shader and render.effect_compile_failed.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the local GStreamer prefix on PKG_CONFIG_PATH); cargo test -p sub-render (42 unit + 1 doc + the golden suites) and cargo test -p sub-plugin (152 tests) both pass. The new tests/effect_golden.rs runs on lavapipe here and skips cleanly where no adapter exists, as the other golden tests do.

Left to TASK-88: showing the reported failure in the inspector and storing per-clip effect chains in the project model; this task stops at the compositor's EffectSource hook and the failure report.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Shader effects now run in the compositor from plugin declarations: sub-render's new effect module validates an effect-desc, derives a 16-byte-per-parameter uniform layout from its ParamDesc list, compiles prelude+plugin WGSL and caches the pipeline (and any compile failure) under a blake3 key, and Compositor::render_with_effects applies each clip's chain in declaration order through ping-pong targets before transform and opacity. An effect whose WGSL will not compile is disabled with the compiler's message in FrameSummary::effect_failures while the clip and the rest of its chain still render. sub-plugin's new optional render feature lifts the WIT effect-types records into those compositor types. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and cargo test for sub-render and sub-plugin, including tests/effect_golden.rs, which composites a reference tint on a grey clip and compares the readback with the mix computed in linear light, plus order-sensitivity and broken-shader cases.
<!-- SECTION:FINAL_SUMMARY:END -->
