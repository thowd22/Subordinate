# Subordinate plugin interfaces

WIT definitions for `subordinate:plugin`. Versioned; the host keeps
compatibility shims for older versions.

## Files

| file | package | worlds |
| --- | --- | --- |
| `subordinate-plugin.wit` | `subordinate:plugin@0.1.0` | `command`, `analyzer` |

`command` is the first and simplest world: a plugin that edits a project by
calling back into the Command API host import (`command-api`), never by holding
the model.

`analyzer` is background analysis (TASK-79): it exports
`analyze(media, options)` and adds the `analysis-host` import, the progress and
cancellation channel of the job the host runs it as. It reports markers, ranges
and metadata in media time and edits nothing; storing the findings and turning
them into markers are ordinary undoable commands (`media.set_analysis`,
`marker.from_analysis`).

The effect, audio-effect, importer, exporter and mcp-tools worlds (TASK-76 to
TASK-81) build on the same `command-api` import.

Guests are plain `cargo build --target wasm32-wasip2` crates using
`wit_bindgen::generate!`; that target emits a component directly, so no
`cargo component` or `wasm-tools component new` step is needed. See
`spikes/wasm-command-world` and backlog doc `doc-2` (the TASK-9 findings).

Licensed under MIT OR Apache-2.0 (see LICENSE-MIT and LICENSE-APACHE) so that
plugins targeting these interfaces may use any licence.
