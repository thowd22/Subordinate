# Subordinate plugin interfaces

WIT definitions for `subordinate:plugin`. Versioned; the host keeps
compatibility shims for older versions.

## Files

| file | package | worlds |
| --- | --- | --- |
| `subordinate-plugin.wit` | `subordinate:plugin@0.1.0` | `command`, `effect`, `effect-cpu` |

`command` is the first and simplest world: a plugin that edits a project by
calling back into the Command API host import (`command-api`), never by holding
the model. The audio-effect, importer, exporter, analyzer and mcp-tools worlds
(TASK-77 to TASK-81) build on the same import.

`effect` is a GPU effect. A WASM guest cannot touch the GPU, so the plugin
*declares* the effect instead of running it: `describe() -> effect-desc` returns
a parameter schema (float, int, bool, colour or a closed set of choices, each
with its range and default) plus WGSL source and the name of its fragment entry
point, and the core compiles, caches and runs the shader (decision-6). It is
called at load time and after a hot reload, never per frame.

`effect-cpu` is `effect` plus an optional `process-cpu(frame, params)` export
for small-buffer work. **It is slow**: the frame is copied into the sandbox,
looped over in WASM and copied back. Use it for thumbnails, analysis passes and
test fixtures, never for playback or export at picture size. A plugin that only
ships a shader targets `effect` and exports nothing else.

Guests are plain `cargo build --target wasm32-wasip2` crates using
`wit_bindgen::generate!`; that target emits a component directly, so no
`cargo component` or `wasm-tools component new` step is needed. See
`spikes/wasm-command-world` and backlog doc `doc-2` (the TASK-9 findings).

Licensed under MIT OR Apache-2.0 (see LICENSE-MIT and LICENSE-APACHE) so that
plugins targeting these interfaces may use any licence.
