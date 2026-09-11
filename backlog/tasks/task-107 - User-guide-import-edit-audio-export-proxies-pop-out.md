---
id: TASK-107
title: 'User guide: import, edit, audio, export, proxies, pop-out'
status: Done
assignee:
  - '@opus-task-107'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 17:19'
labels:
  - docs
milestone: m-7
dependencies:
  - TASK-62
  - TASK-70
  - TASK-67
references:
  - docs/PLAN.md
priority: medium
ordinal: 128000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users need a concise guide covering the MVP feature set.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 docs/user-guide.md with one section per MVP feature and screenshots
- [x] #2 Keyboard shortcut reference generated from the action registry
- [x] #3 Troubleshooting section for missing hardware encoders
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add shortcuts::reference_markdown(), a platform-neutral Markdown table generated from Action::ALL/Category::ALL and a ShortcutMap, plus an example binary (cargo run -p sub-ui --example shortcut-reference) that prints it for regeneration.
2. Write docs/user-guide.md: one section per MVP feature (import and the bin, editing the timeline, audio, export, proxies, pop-out viewer), each illustrated with the committed egui_kittest snapshot PNGs under crates/sub-ui/tests/snapshots so the screenshots are re-recorded by CI rather than hand-captured.
3. Embed the generated shortcut reference between <!-- shortcuts:begin/end --> markers.
4. Add a troubleshooting section for missing hardware encoders: Hardware diagnostics panel, subordinate-cli diag, the selection order and software fallback, the per-vendor install hints from sub_media::diagnostics.
5. Add crates/sub-ui/tests/user_guide.rs (the docs/mcp-guide.md test pattern): the embedded block equals the generated one, every Action id and label appears, every referenced screenshot file exists, every required section is present.
6. Link the guide from README.md; run fmt, clippy and the sub-ui tests (GStreamer env sourced).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
docs/user-guide.md is the guide: starting up, the window, importing media, editing, audio, export, proxies, the pop-out viewer, the keyboard reference and troubleshooting. Every claim was read out of the code it describes (panel modules in crates/sub-ui/src, sub-export presets and encoder catalogue, sub-media proxy and diagnostics), so the guide names the labels the editor actually shows: Import..., Reset layout, Pop out viewer, Automatic, the five proxy badges.

Screenshots are the committed egui_kittest references under crates/sub-ui/tests/snapshots rather than hand-captured PNGs: CI re-records them whenever a panel changes, so the guide's pictures cannot drift from the build and no image bytes are duplicated in the repository.

The keyboard reference is generated: shortcuts::reference_markdown() walks Action::ALL by Category and writes the chord, the label and the stable action id from a ShortcutMap, using platform-independent modifier names (portable_chord_label) because a document is read on machines other than the one it was written on. It lands between <!-- shortcuts:begin/end --> markers and is re-recorded with UPDATE_DOCS=1 cargo test -p sub-ui --test user_guide, a flow documented in docs/DEVELOPMENT.md next to the snapshot one.

crates/sub-ui/tests/user_guide.rs is the drift guard, following the docs/mcp-guide.md test pattern: the embedded block equals the generated one, every action appears with its id, label and chord, every screenshot link is an existing committed snapshot, every required section is present, and the troubleshooting section names every element in sub_export's encoder catalogue. Two unit tests in shortcuts.rs cover the generator itself, including that an unbound action is still listed.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui --lib --test user_guide -> 371 + 5 passed (GStreamer env sourced). The GPU snapshot tests were not re-run: nothing here changes a panel, and no snapshot was re-recorded.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added docs/user-guide.md covering the MVP feature set — import, editing, audio, export, proxies and the pop-out viewer — illustrated with the committed egui_kittest snapshot references so the screenshots are re-recorded by CI, plus a keyboard reference generated from the action registry (shortcuts::reference_markdown, re-recorded with UPDATE_DOCS=1) and a troubleshooting section for missing hardware encoders that names the whole selection order and the per-platform install steps. crates/sub-ui/tests/user_guide.rs fails the build when any of it goes stale. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-ui --lib --test user_guide (371 + 5 passed).
<!-- SECTION:FINAL_SUMMARY:END -->
