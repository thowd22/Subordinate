---
id: decision-1
title: 'Use Rust with GStreamer, wgpu, egui, wasmtime and rmcp as the core stack'
date: '2026-09-08 20:52'
status: accepted
---
## Context

The MVP needs a memory-safe systems language with maintained bindings for media decode/encode, GPU compositing, audio, WASM hosting and MCP on Linux, Windows and macOS. Research on 2026-09-08 found gstreamer-rs 0.25 is the only safe-Rust route to NVENC, VA-API, AMF, VideoToolbox and Media Foundation; ffmpeg-next is maintenance-only with unsafe hardware paths.

## Decision

Rust workspace. GStreamer 1.28 via gstreamer-rs for media I/O, wgpu 30 for compositing, egui 0.36 for UI including pop-out viewports, cpal + symphonia + rubato with an owned mixer, wasmtime 48 for plugins, rmcp 3.x for MCP.

## Consequences

One toolchain everywhere. GStreamer runtime must be bundled per platform in CI from day one. Pin wasmtime and rmcp versions because both move fast. See docs/PLAN.md section 3.
