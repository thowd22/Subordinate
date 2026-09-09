---
id: TASK-20
title: NV12 upload and YUV-to-RGB conversion shader
status: Done
assignee:
  - '@opus-task-20'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 04:28'
labels:
  - render
milestone: m-1
dependencies:
  - TASK-19
  - TASK-14
references:
  - docs/PLAN.md
priority: high
ordinal: 41000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decoded frames arrive as NV12; converting on the GPU is the fastest path until zero-copy import lands.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Two-plane texture upload for NV12 with a WGSL shader producing RGBA with Rec.709 limited-range conversion
- [x] #2 Handles odd dimensions and stride padding
- [x] #3 A golden test compares a converted colour-bar frame against reference pixel values within tolerance
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-render/src/nv12.rs: Nv12Geometry (width/height/strides, odd-size chroma rounding, plane length + slice checks) promoted from the TASK-7 spike with render.* error codes.
2. Add Nv12Converter: R8Unorm luma + Rg8Unorm chroma textures, strided write_texture uploads (no CPU repack), fullscreen-triangle WGSL pass writing Rgba8UnormSrgb, Rec.709 limited-range (16-235 / 16-240) matrix.
3. Extend RenderError with BadGeometry (render.bad_geometry) and ShortPlane (render.short_plane).
4. Unit tests for geometry maths incl. odd dimensions and decoder stride padding.
5. Golden test tests/nv12_golden.rs: synthesise an 8-bar Rec.709 colour-bar NV12 frame (odd size + padded strides variants), convert on the headless context, read back and compare against reference RGB within tolerance; skip cleanly when no adapter.
6. Verify with cargo fmt --check, clippy -D warnings, cargo test -p sub-render.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-render/src/nv12.rs, promoting the TASK-7 spike's geometry maths into production with render.* error codes.

Design points:
- Upload is two write_texture calls (R8Unorm luma, Rg8Unorm chroma) using the decoder's own strides as bytes_per_row, so padded planes are copied verbatim with no CPU repack; the copy extent stays at the picture size, so padding bytes never reach the texture (the golden test fills padding with 0xA5 to prove it).
- The fragment shader textureLoads luma at the fragment's pixel coordinate (exact, immune to odd widths) and bilinearly samples chroma at (x+0.5)/2 divided by textureDimensions(chroma), which keeps centre-sited 4:2:0 upsampling correct when an odd width or height rounds the chroma plane up.
- Rec.709 limited range: luma (Y-16)/219, chroma (C-128)/224, matrix 1.5748 / -0.1873 / -0.4681 / 1.8556, clamped to 0..1.
- The pass then applies the sRGB EOTF before writing to the Rgba8UnormSrgb target, whose encode on store is its exact inverse: the texture holds linear light for later compositing while a sampler returns the decoder's gamma-encoded values.
- Output texture also carries COPY_SRC so the golden test (and later export readback) can copy it back.
- RenderError gained BadGeometry (render.bad_geometry) and ShortPlane (render.short_plane); short planes are refused before anything is uploaded.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (exit 0, with the local GStreamer sysroot on PKG_CONFIG_PATH); cargo test -p sub-render green (15 unit + 3 headless-context + 4 golden). The golden tests ran on a real device (Mesa software Vulkan) on this machine, not skipped. The tolerance was checked to be meaningful: swapping Cb and Cr in the shader makes all three colour-bar tests fail (green 229 vs expected 255).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added sub-render::nv12 with Nv12Geometry (stride- and odd-size-aware NV12 layout, plane length checks) and Nv12Converter (two-plane R8Unorm/Rg8Unorm upload at the decoder's strides plus a fullscreen WGSL pass converting Rec.709 limited range to an Rgba8UnormSrgb texture), and two new RenderError codes render.bad_geometry and render.short_plane. Verified with cargo test -p sub-render: six geometry unit tests plus a golden test (crates/sub-render/tests/nv12_golden.rs) that converts a nine-bar Rec.709 colour-bar frame packed, stride-padded (192-byte rows) and odd-sized (145x33), reads the result back from the GPU and compares every bar interior against reference RGB within 3 codes; the tolerance was proven meaningful by making a deliberate Cb/Cr swap fail the test. cargo fmt --all --check and cargo clippy --workspace --all-targets -- -D warnings are clean.
<!-- SECTION:FINAL_SUMMARY:END -->
