---
id: TASK-86
title: Hot reload with --dev install and file watching
status: Done
assignee:
  - '@opus-task-86'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 08:02'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-85
references:
  - docs/PLAN.md
priority: high
ordinal: 107000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The agent development loop depends on fast reload (§6.4).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 plugin install --dev symlinks or watches the built .wasm and reloads within a second of change
- [x] #2 Reload preserves engine state and re-registers commands, effects and tools
- [x] #3 Reload errors are returned to the CLI and MCP caller as structured SubError JSON
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-plugin: new `dev` module. Dev install links a source plugin directory into a plugin directory: plugin.toml and the built plugin.wasm are symlinked when the platform allows and copied otherwise, with a dev.json recording the sources and the link mode. install() also serves a plain (copy) install.
2. Watching: a dependency-free polling DevWatcher fingerprints (len, mtime) of each dev plugin's manifest and wasm, debounces a half-written file and reports a change; spawn() runs it on a thread at a 200 ms poll so a rebuild is seen well inside a second.
3. Reload: DevHost holds the PluginRegistry, a host-supplied loader closure and the artifacts each plugin contributed (commands, effects, MCP tools). reload() refreshes copies, re-runs the loader, and on success swaps the plugin's PluginCommandRegistry entries, effects and tool catalog; on failure the previous registration and all engine state stay exactly as they were and the failure is recorded as a ReloadStatus and returned as a SubError with a stable code.
4. Stable codes: plugin.dev_source_invalid, plugin.install_failed, plugin.install_conflict, plugin.not_dev_installed.
5. Command API: dev::register_methods adds plugin.install and plugin.reload; docs/schema/plugin-api.json regenerated. InstalledPlugin gains a dev flag so a listing (and TASK-98's panel) can tell a dev install apart.
6. CLI: subordinate-cli plugin install <path> [--dev] and plugin reload <id>, JSON out like the rest; serve registers the new methods and runs the watcher so a dev plugin reloads while it is serving.
7. Tests: install link/copy round trip, watcher sees a rewritten wasm inside a second, reload re-registers commands and keeps a failed reload's previous registration, structured error for a corrupt wasm, CLI subcommand tests. Then fmt, clippy -D warnings, cargo test for the touched crates.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in one new module plus its CLI and serve wiring.

**sub-plugin/src/dev.rs.** `DevSource::resolve` accepts what PLAN §6.4 tells an agent to type — the built .wasm, whose plugin.toml is found beside it or up to eight directories above it, or the plugin directory itself. `install(dirs, location, source, dev)` puts the plugin in the user or project-local directory: with --dev every top-level entry of the source tree (so a manifest's MCP schema files come along) plus the built component are symlinked in, the component under the one name the host loads it by, `plugin.wasm`; a `dev.json` records the sources and the link mode, and its presence is what `InstalledPlugin.dev` reports. Windows may refuse a symlink to an unprivileged process, so each entry falls back to a copy and `DevInstall::refresh` re-copies before every reload — the reload path is identical either way. Installing over the same plugin replaces it; over a different one it is refused with plugin.install_conflict.

**Watching without a new dependency.** `DevWatcher` polls (length, mtime) of each dev plugin's manifest and component. A change is reported only after the fingerprint has held still for one further poll, so a half-written .wasm is never handed to the loader; at the 200 ms POLL_INTERVAL a rebuild lands in about 400 ms. `sync(&Scan)` re-derives the watched set from the registry, taking a newly seen plugin up with no baseline so its first settled reading is a change — that is what loads dev plugins at startup and closes the race where a rebuild lands between the install and the first look. `watch_and_reload(host, interval)` is the thread the editor and `serve` run; dropping the handle stops it.

**Reload.** `DevHost` holds the registry, a host-supplied loader (`Fn(&InstalledPlugin, &Path) -> SubResult<PluginArtifacts>`; the host is what knows how to instantiate a component, so this crate does not presume it) and, per plugin, the commands, effect declarations and MCP tools that load contributed. `reload` scans, refreshes a copied install, runs the loader, builds the new ToolCatalog and command registration on the side, and only then swaps them in — so nothing can be half-replaced. It never touches the engine, the project or the undo stack. A failure leaves the previous version registered and running, records a ReloadStatus carrying the flattened code/message/details, and returns the SubError.

**Surfaces.** New stable codes: plugin.dev_source_invalid, plugin.install_failed, plugin.install_conflict. `dev::register_methods` adds plugin.install, plugin.reload and plugin.status to the Command API; they are folded into the same generated document, so docs/schema/plugin-api.json (regenerated) gives subordinate-mcp the matching MCP tools with no hand-written list. `subordinate-cli plugin install <path> [--dev] [--project-local]` and `plugin reload <id>` use a loader that only compiles the component — the CLI runs no plugins — which is enough to answer a bad build with the structured error. `serve` registers the three methods and runs the watcher for as long as it serves.

**Deliberately not done here:** the plugins panel of AC #3 is TASK-98's; the data it needs is ready (DevHost::statuses, plugin.status, ReloadStatus.error). Instantiating a real component to harvest its commands/effects/tools exports is the editor's loader (TASK-96/TASK-98 wire it); this crate defines the seam and ships the compile-only loader.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test for sub-plugin, subordinate-cli and subordinate-mcp — 24 test binaries, all green, including tests/hot_reload.rs which dev-installs a plugin, runs watch_and_reload, rewrites the component and asserts the reload lands inside one second with the commands replaced, that a broken rebuild leaves the running version registered with plugin.load_failed recorded, and that a reload leaves a real sub_edit::Engine's project and undo/redo stack untouched. Manual smoke with a real WASM component (the counter test guest): plugin install --dev reported mode "symlink" and a successful load, plugin reload reloaded it, and after overwriting the .wasm with garbage plugin reload printed the JSON SubError plugin.load_failed on stderr and exited 1. A full cargo test --workspace was not run to completion here — the GUI crates' test build exceeds the time available in this environment — but cargo clippy --workspace --all-targets compiles every test target cleanly.

2026-09-10: the 'shown in a plugin panel' half moved to TASK-98, which builds that panel.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Dev install with file watching and hot reload preserving engine state; reload errors surface to CLI and MCP callers. Panel display is TASK-98.
<!-- SECTION:FINAL_SUMMARY:END -->
