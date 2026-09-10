# Subordinate plugin interfaces

WIT definitions for `subordinate:plugin`. Versioned; the host keeps
compatibility shims for older versions.

## Files

| file | package | worlds |
| --- | --- | --- |
| `subordinate-plugin.wit` | `subordinate:plugin@0.1.0` | `command`, `commands`, `effect`, `effect-cpu`, `audio-effect`, `importer`, `exporter`, `analyzer`, `mcp-tools` |

`command` is the first and simplest world: a plugin that edits a project by
calling back into the Command API host import (`command-api`), never by holding
the model. Every other world builds on that same import.

`commands` is the same world with menu and shortcut registration: the plugin
answers `commands()` with a `command-desc` per entry it contributes — an id, a
title and optionally the chord it would like — and the host puts those in the
Plugins menu and the shortcut registry, then calls `run(id, context)` when one
is chosen. The host opens one undo group around a run, so a plugin command is a
single step on the undo stack however many primitives it applies.

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

`importer` and `exporter` contribute media import specs and export presets:
they describe what they can handle and hand the host specs it applies itself,
so the project only ever changes through ordinary commands.

`analyzer` is background analysis (TASK-79): it exports
`analyze(media, options)` and adds the `analysis-host` import, the progress and
cancellation channel of the job the host runs it as. It reports markers, ranges
and metadata in media time and edits nothing; storing the findings and turning
them into markers are ordinary undoable commands (`media.set_analysis`,
`marker.from_analysis`).

Guests are plain `cargo build --target wasm32-wasip2` crates using
`wit_bindgen::generate!`; that target emits a component directly, so no
`cargo component` or `wasm-tools component new` step is needed. See
`spikes/wasm-command-world` and backlog doc `doc-2` (the TASK-9 findings).

Licensed under MIT OR Apache-2.0 (see LICENSE-MIT and LICENSE-APACHE) so that
plugins targeting these interfaces may use any licence.
