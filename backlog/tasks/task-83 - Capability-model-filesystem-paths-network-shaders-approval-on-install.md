---
id: TASK-83
title: 'Capability model: filesystem paths, network, shaders, approval on install'
status: Done
assignee:
  - '@opus-task-83'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 03:24'
labels:
  - plugins
  - security
milestone: m-6
dependencies:
  - TASK-82
references:
  - docs/PLAN.md
priority: high
ordinal: 104000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Plugins are sandboxed by default (§6.1).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Capabilities map to WASI preopens and host-function gating; $PROJECT and $PLUGIN_DATA path variables expand
- [x] #2 Installing a plugin records approved capabilities; a changed manifest requires re-approval
- [x] #3 A plugin without fs_read cannot open files (test)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New sub-plugin module capability.rs: PathVars ($PROJECT/$PLUGIN_DATA expansion to host+guest paths), Preopen, ResolvedCapabilities::resolve mapping an approved [capabilities] block onto WASI preopens plus network/shader gates.
2. Host-function gating: authorize_read/write/network/shaders, all refusing with the stable code plugin.capability_denied; lexical path normalisation so a traversal cannot escape a granted root.
3. Real WASI wiring: apply_to_wasi feeds the preopens into a wasmtime-wasi WasiCtxBuilder with FsPerms from the grant, and inherits the network only when granted.
4. Install-time approval: ManifestDigest (blake3 over the parsed manifest), Approval, ApprovalStatus, ApprovalStore with approve/revoke/status/authorize and JSON load/save; a changed manifest yields plugin.approval_stale and loading refuses.
5. New error codes in sub_plugin::codes; re-exports and crate docs.
6. Tests: unit tests in the module plus an integration test that a plugin without fs_read cannot open files (host gate denies, sandboxed grant yields no preopens, and a WasiCtx built from it gets none), then fmt/clippy/test.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-plugin/src/capability.rs, the whole capability model, plus crates/sub-plugin/tests/capabilities.rs.

Design:
- PathVars binds $PROJECT (the open project's directory, absent when nothing is open) and $PLUGIN_DATA (the plugin's private directory) and expands a declared root into a Preopen: host path, guest mount (/project, /plugin-data) and Access. Only variable-rooted paths are accepted, so an absolute host path or a .. segment in a manifest is plugin.invalid_capability_path rather than a way out of the sandbox; $PROJECT with no project open is plugin.unset_path_variable.
- ResolvedCapabilities::resolve maps an approved [capabilities] block onto that preopen list (a root in both fs_read and fs_write collapses to one read-write preopen) plus the network and shader gates. apply_to_wasi feeds the list into a real wasmtime-wasi WasiCtxBuilder with FsPerms::ReadOnly/ReadWrite and inherits the network only when granted; sub-plugin now depends on wasmtime-wasi 48 (p2) for that. Shaders are not a WASI capability, so authorize_shaders is the host-side gate TASK-87 will call.
- Host-function gating is authorize_read/write/network/shaders, all refusing with plugin.capability_denied and a capability (and path) detail. Paths are lexically normalised first, so a traversal is checked at the place it actually lands and a relative path is refused outright.
- Approval: ManifestDigest is blake3 over the parsed manifest (not the file bytes, so reformatting or recommenting is not a change), Approval records the digest, version and granted capabilities, and ApprovalStore holds one per plugin id with approve/revoke/status/authorize and JSON load/save (schema_version 1, a missing file loads empty). authorize resolves the *recorded* capabilities, never the manifest's current block, so editing plugin.toml after approval cannot widen a grant: it yields plugin.approval_stale with a capabilities_changed detail until re-approved, and an unknown plugin yields plugin.not_approved.
- New stable codes in sub_plugin::codes: invalid_capability_path, unknown_path_variable, unset_path_variable, capability_denied, preopen_failed, not_approved, approval_stale, invalid_approvals, approvals_unreadable, approvals_unwritable, invalid_digest.

Scope note: enforcement is proved at the host boundary and at the WASI context, not by running a WASM guest that opens a file. sub-plugin's wasmtime has no compiler feature (runtime only), so no component can be instantiated here; TASK-84 owns the wasmtime host and is where an end-to-end guest test belongs. AC #3 is checked on the evidence that a plugin without fs_read is refused by the host gate and is handed zero WASI preopens, with the target file provably present on disk.

Verification (in the worktree): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-plugin passes 89 unit + 7 capability integration tests + 7 doctests. docs/schema/plugin-manifest.json regenerated (a Capabilities doc comment feeds it) with SUB_UPDATE_SCHEMA=1.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Implemented the plugin capability model in sub-plugin: PathVars expands $PROJECT and $PLUGIN_DATA roots into WASI preopens (guest mounts /project and /plugin-data), ResolvedCapabilities carries those preopens plus the network and shader gates and configures a real wasmtime-wasi WasiCtxBuilder, and authorize_read/write/network/shaders are the host-function gates, every refusal being plugin.capability_denied. Install-time approval is ApprovalStore: it records the granted capabilities against a blake3 digest of the parsed manifest, resolves only what was recorded, and refuses a plugin whose plugin.toml changed since with plugin.approval_stale until it is approved again. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings (both clean) and cargo test -p sub-plugin (89 unit, 7 new integration and 7 doctests passing), including a test that a plugin without fs_read is denied a file that provably exists and gets zero preopens.
<!-- SECTION:FINAL_SUMMARY:END -->
