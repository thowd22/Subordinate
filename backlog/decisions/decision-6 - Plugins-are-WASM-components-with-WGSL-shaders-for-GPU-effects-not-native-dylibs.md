---
id: decision-6
title: >-
  Plugins are WASM components with WGSL shaders for GPU effects, not native
  dylibs
date: '2026-09-08 20:52'
status: accepted
---
## Context

Plugins must be sandboxed, language-agnostic and easy for coding agents to produce. WASM cannot touch the GPU and per-pixel loops in WASM are too slow at 4K.

## Decision

Plugins are WASM components with versioned WIT worlds hosted by wasmtime, following Zed's pattern. GPU effects are declared as WGSL shaders plus a parameter schema that the core compiles and runs. A native dylib tier for OpenFX and frei0r bridges is post-MVP.

## Consequences

Plugin authors can use Rust, Go, Python or JS. Heavy pixel work must be expressed as shaders. Host keeps compatibility shims per WIT version.
