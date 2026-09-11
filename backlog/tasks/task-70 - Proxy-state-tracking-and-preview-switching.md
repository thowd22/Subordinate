---
id: TASK-70
title: Proxy state tracking and preview switching
status: Done
assignee:
  - '@opus-task-70'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 10:38'
labels:
  - media
  - ui
milestone: m-5
dependencies:
  - TASK-69
  - TASK-59
references:
  - docs/PLAN.md
priority: high
ordinal: 91000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Preview uses proxies, export never does.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 MediaItem proxy state (none, generating, ready, stale) is shown in the bin
- [x] #2 Viewer toggle uses proxies when ready; export pipeline always uses originals (test asserts)
- [x] #3 Proxies are invalidated when the source content hash changes
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the proxy indicator and preview switch: a committed snapshot of each proxy state and an interaction test for the switch
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-model: extend ProxyState with Stale, add label/is_ready helpers, and a MediaUse/MediaSource resolution API where Preview honours a proxy toggle and Export always returns the original.
2. sub-model: MediaItem::invalidate_proxy turns a Ready proxy Stale when the source content hash changes.
3. sub-edit: new SetProxyState command (media.set_proxy) with a restoring inverse; RelinkMedia carries the previous proxy state and invalidates a Ready proxy when the hash it records differs.
4. sub-ui media bin: a proxy badge per item in the list and grid views showing none/generating/ready/stale/failed.
5. sub-ui viewer: a Proxy toggle in the transport row plus preview source resolution through MediaUse::Preview.
6. Tests: sub-model unit tests (export ignores a ready proxy, staleness), sub-edit command tests, and a sub-ui egui_kittest file on the shared harness with a committed snapshot per proxy state and an interaction test for the viewer switch.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Model: ProxyState gains a Stale(MediaPath) variant alongside None/Pending(generating)/Ready/Failed, plus label(), is_ready/is_stale/is_generating, file() (the proxy on disk, ready or stale) and invalidated(). New MediaUse { Preview { proxies }, Export } and MediaSource { path, is_proxy } make source selection a single branch: MediaUse::Export has no proxy arm at all, so 'export never uses a proxy' is one place a test can pin. MediaItem::source/absolute_source and Project::absolute_source read it; MediaItem::invalidate_proxy turns Ready into Stale.

Edit: new SetProxyState command (kind media.set_proxy, registered in the builtin registry, so it reaches the Command API and MCP) with an inverse that restores the previous state. RelinkMedia now carries the previous proxy state in its inverse and invalidates a Ready proxy whenever the hash it records differs from the item's - which covers both a relink to other bytes and a re-hash of a file changed in place. A relink to the same hash keeps the proxy.

UI: the media bin draws a proxy badge on every list row and grid tile (NO PROXY / PROXY... / PROXY / PROXY STALE / PROXY FAILED, with hover text for ready, stale and failed). The viewer transport row gains a 'Proxy' toggle (on by default) and ViewerPanel::media_use / preview_source resolve which file preview opens; it is view state, not a Command, since which file the preview reads is not an edit.

Regenerated docs/schema/project-v1.schema.json and docs/schema/command-api.json (SUB_UPDATE_SCHEMA=1) and added media_set_proxy to docs/mcp-guide.md, which its guide test requires. Re-recorded the four existing snapshots the badge and the toggle change (media_bin_list, media_bin_grid, bin_drop_target, viewer_frame_ten).

Verification on this machine (GStreamer from the local sysroot, software wgpu adapter): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test --workspace green after the guide fix; crates/sub-ui/tests/proxy_preview.rs passes with the committed snapshots (no UPDATE_SNAPSHOTS).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Proxy state is now tracked end to end and preview can switch to it while export cannot. ProxyState gained a Stale variant and the model gained MediaUse/MediaSource, the single place that decides which file a media item is read from: preview may take a ready proxy, export always takes the original. A new SetProxyState command records what proxy jobs report, and RelinkMedia invalidates a ready proxy whenever the source hash changes, undoably. The media bin shows a badge for every proxy state and the viewer transport row carries a Proxy toggle. Verified with cargo fmt --check, clippy -D warnings, cargo test --workspace, and crates/sub-ui/tests/proxy_preview.rs: five committed egui_kittest snapshots (one per state) plus interaction tests that click the toggle and assert preview switches file while MediaUse::Export never does.
<!-- SECTION:FINAL_SUMMARY:END -->
