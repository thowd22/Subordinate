# Subordinate plugin interfaces

WIT definitions for `subordinate:plugin`. Versioned; the host keeps
compatibility shims for older versions.

## Files

| file | package | worlds |
| --- | --- | --- |
| `subordinate-plugin.wit` | `subordinate:plugin@0.1.0` | `command`, `commands` |

`command` is the first and simplest world: a plugin that edits a project by
calling back into the Command API host import (`command-api`), never by holding
the model. The effect, audio-effect, importer, exporter, analyzer and mcp-tools
worlds (TASK-75 to TASK-81) build on the same import.

`commands` is the same world with menu and shortcut registration: the plugin
answers `commands()` with a `command-desc` per entry it contributes — an id, a
title and optionally the chord it would like — and the host puts those in the
Plugins menu and the shortcut registry, then calls `run(id, context)` when one
is chosen. The host opens one undo group around a run, so a plugin command is a
single step on the undo stack however many primitives it applies.

Guests are plain `cargo build --target wasm32-wasip2` crates using
`wit_bindgen::generate!`; that target emits a component directly, so no
`cargo component` or `wasm-tools component new` step is needed. See
`spikes/wasm-command-world` and backlog doc `doc-2` (the TASK-9 findings).

Licensed under MIT OR Apache-2.0 (see LICENSE-MIT and LICENSE-APACHE) so that
plugins targeting these interfaces may use any licence.
