---
id: TASK-100
title: 'First-party plugin: colour tint and exposure effect (WGSL)'
status: Done
assignee:
  - '@opus-task-100'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 18:44'
labels:
  - plugins
  - first-party
milestone: m-6
dependencies:
  - TASK-87
  - TASK-91
references:
  - docs/PLAN.md
priority: medium
ordinal: 121000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Reference effect plugin proving the shader path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Effect with tint colour, exposure and saturation params
- [x] #2 Golden render test passes
- [x] #3 Documented as the effect template
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New first-party plugin crate plugins/color: own one-package workspace like plugins/gain, cdylib+rlib, subordinate-sdk with default-features=false features=["effect"], plus a plugin.toml declaring worlds=["effect"] and capabilities.shaders.
2. plugins/color/src/grade.rs: WIT-free parameter table (tint colour, tint_amount, exposure in stops, saturation) plus the CPU reference implementation of the grade, unit-tested on the host triple.
3. plugins/color/src/effect.wgsl: fragment entry fs_main applying exposure as exp2(stops), a multiplicative tint mixed by amount and Rec.709 saturation in linear light, alpha untouched; defaults are the identity grade.
4. plugins/color/src/lib.rs: describe() lifts grade::PARAMS into effect-types param-desc records and include_str!s the WGSL; tests assert the declaration matches the table.
5. Golden render test crates/sub-render/tests/color_plugin_golden.rs: pulls in the plugin's own WGSL with include_str! and its grade.rs with #[path] (no dependency edge into the plugin workspace), runs the effect through the compositor on a flat clip and compares the readback with the CPU reference in linear light; identity, tint, exposure, saturation and combined cases.
6. Document it as the effect template: plugins/color/CLAUDE.md written like the scaffold's, pointers from the SDK/sub-plugin docs and README, and a CI step that builds the component for wasm32-wasip2 and runs its tests, next to the gain one.
7. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, cargo test -p sub-render, plus fmt/test inside plugins/color.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
plugins/color is the first-party effect plugin: id com.subordinate.color, one WASM component on subordinate-sdk with default-features = false, features = ["effect"], in a one-package workspace of its own like plugins/gain so it stays out of the host workspace's build and lint graph. plugin.toml asks for capabilities.shaders and nothing else.

It is deliberately three files, and that split is the template's real content. src/grade.rs is the parameter table (tint colour, tint_amount, exposure in stops, saturation) plus Grade::apply, a CPU reference for the grade — plain Rust, no WIT, so it is unit-testable on the host triple. src/effect.wgsl is the shader: exposure as exp2(stops), a multiplicative tint mixed in by its amount, saturation towards Rec.709 luma, a floor at zero, alpha untouched, all in linear light because the sampler decodes sRGB and the target re-encodes. src/lib.rs is only the lift from the table into the effect-desc describe() returns.

A fourth parameter beyond the three the criterion names, tint_amount, is there because a tint colour with no strength has no off position; the declared defaults (white tint, amount 0, 0 stops, saturation 1) are the identity grade, so dropping the effect on a clip changes no pixel until a control is moved.

The golden test is crates/sub-render/tests/color_plugin_golden.rs. It reads the plugin's own shader with include_str! and its own parameter table with #[path], so there is no dependency edge into the plugin's workspace and no second copy of either: it builds the same declaration the host builds from describe(), renders a flat 64/128/192 clip through Compositor::render_with_effects and compares the readback with Grade::apply computed in linear light, over six cases (identity, a stop down, greyscale, double saturation, a half-strength warm tint, and all three at once) plus an out-of-range value the host clamps. A parameter renamed, re-ranged or dropped on one side only fails there. It also asserts the uniform layout is one 16-byte slot per parameter in declaration order, which is what catches a colour packed into the wrong slot.

Documented as the effect template in plugins/color/CLAUDE.md (interface, the three-file split, the shader's contract with the host prelude, capabilities, the build/install/test loop), a new 'Reference plugins' section in docs/DEVELOPMENT.md, a pointer in docs/PLAN.md §6.4, one in sub-plugin's effect-world crate docs, and a paragraph in the scaffolded effect CLAUDE.md (bins/subordinate-cli/src/scaffold.rs) pointing a new plugin at it. CI gains a 'Reference plugin (effect)' Linux step next to the gain one: build for wasm32-wasip2, cargo test, cargo fmt --all --check.

Verified here: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the local GStreamer prefix on PKG_CONFIG_PATH); cargo test -p sub-render (all suites, including the new color_plugin_golden's 9 tests on lavapipe); cargo test -p sub-plugin and -p subordinate-cli green after the doc and scaffold edits; and inside plugins/color, cargo test (9 tests), cargo clippy --all-targets -- -D warnings, cargo fmt --all --check and cargo build --release --target wasm32-wasip2, which emits subordinate_plugin_color.wasm.

Not done here: installing the component and running subordinate-cli plugin test against it end to end. The component builds, but a full install-and-run pass belongs with the sample project (TASK-109); nothing in this task's criteria needs it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
plugins/color is the first-party effect plugin and the template for the effect world: a colour grade with a tint colour, a tint amount, exposure in stops and saturation, declared as effect-desc parameters over one WGSL shader the host compiles, caches and runs (decision-6, docs/PLAN.md §6.2). It splits into a WIT-free parameter table with a CPU reference for the grade (src/grade.rs), the shader (src/effect.wgsl) and the lift between them (src/lib.rs), asks only for the shaders capability, and its declared defaults are the identity grade. The golden test crates/sub-render/tests/color_plugin_golden.rs pulls in that same shader with include_str! and the same table with #[path] — no dependency edge into the plugin's own workspace — renders a flat clip through the compositor and compares the readback with the plugin's reference in linear light across identity, exposure, saturation, tint and combined cases, plus uniform-layout and range-clamping checks. Documented as the template in plugins/color/CLAUDE.md, a new Reference plugins section of docs/DEVELOPMENT.md, docs/PLAN.md §6.4, sub-plugin's effect-world docs and the scaffolded effect CLAUDE.md. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test for sub-render, sub-plugin and subordinate-cli, and inside plugins/color cargo test, cargo clippy, cargo fmt and a wasm32-wasip2 component build.
<!-- SECTION:FINAL_SUMMARY:END -->
