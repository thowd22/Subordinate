---
id: TASK-124
title: 'UI testing convention: every panel task ships a kittest snapshot'
status: Done
assignee:
  - '@opus-task-124'
created_date: '2026-09-09 18:21'
updated_date: '2026-09-10 08:31'
labels:
  - ui
  - test
  - docs
milestone: m-2
dependencies:
  - TASK-119
priority: medium
ordinal: 144000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Without a convention, new panels will land untested again. Encode the rule where agents read it and make the harness easy enough that the rule is cheap to follow.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 CLAUDE.md conventions list states that any task touching sub-ui adds or updates a kittest snapshot or interaction test, and names the harness module
- [x] #2 docs/DEVELOPMENT.md has a UI testing section covering the harness, snapshot update, artifact inspection for agents, and the rule that GPU runners are only for GPU-specific checks
- [x] #3 Open UI tasks in m-2 and m-5 (inspector, crossfade UI, markers, docking, pop-out, fullscreen, effect UI, plugin panel, export panel) have a snapshot acceptance criterion appended
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a conventions bullet to CLAUDE.md (and mirror into AGENTS.md if it carries the same list): any task touching sub-ui adds or updates a kittest snapshot or interaction test, naming crates/sub-ui/tests/support/mod.rs as the harness module.
2. Extend the existing 'UI tests (egui_kittest)' section in docs/DEVELOPMENT.md so it explicitly covers all four required points: the harness module and how to use it, how to update snapshots, how an agent inspects the CI diff artifact (gh run download of ui-snapshot-diffs-<os>) and the local .new/.diff PNGs, and the rule that hosted runners carry the software adapters so GPU runners are reserved for GPU-specific checks.
3. Append a snapshot/interaction-test acceptance criterion to every open UI panel task named by AC#3: m-2/m-5 ui tasks (29, 30, 31, 32, 37, 38, 40, 43, 67, 68, 70, 71, 111) plus the explicitly named effect UI (88), plugin panel (98) and export panel (62).
4. Add a guard test in crates/sub-ui/tests/ui_harness.rs asserting the convention text is present in CLAUDE.md and docs/DEVELOPMENT.md, so the rule cannot silently vanish.
5. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test -p sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
AC#1: CLAUDE.md gains a 'UI tests' conventions bullet naming the harness module crates/sub-ui/tests/support/mod.rs, the snapshot directory and the re-record command.

AC#2: docs/DEVELOPMENT.md 'UI tests (egui_kittest)' section rewritten with the rule stated up front and four subsections: 'The harness' (entry points plus a worked example), 'Updating snapshots' (UPDATE_SNAPSHOTS=1, tolerances, size budget), 'Looking at a failure (including from an agent)' (local .new/.diff PNGs plus gh run download of the ui-snapshot-diffs-<os> artifact and how to read a diff), and 'Where these run' (hosted runners carry the software adapters; GPU runners are only for checks that need real hardware).

AC#3: appended a snapshot/interaction-test acceptance criterion to the 16 open UI tasks named by the criterion: TASK-29, 30, 31, 32, 37 (inspector), 38 (crossfade), 40 (markers), 43 (docking), 62 (export panel), 67 (pop-out), 68 (fullscreen), 70, 71, 88 (effect UI), 98 (plugin panel), 111. TASK-120/121/122 were skipped: they are themselves the snapshot-test tasks. TASK-118 was skipped: it is the manual GPU verification task, which the new docs explicitly route away from kittest.

Also added the regression guard the_ui_test_convention_is_documented_where_agents_read_it in crates/sub-ui/tests/ui_harness.rs, so deleting either half of the convention fails the build rather than passing silently.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui all green (ui_harness 6 passed, including the new guard, plus every panel suite and 8 doctests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Encoded the UI testing rule where agents read it. CLAUDE.md now carries a 'UI tests' conventions bullet naming the shared harness (crates/sub-ui/tests/support/mod.rs), and docs/DEVELOPMENT.md's 'UI tests (egui_kittest)' section was rewritten to cover the harness entry points with a worked example, snapshot re-recording and tolerances, how an agent inspects a failure locally and through the ui-snapshot-diffs-<os> CI artifact, and the rule that hosted runners run every UI test while GPU runners are reserved for hardware-specific checks. Sixteen open UI tasks (TASK-29/30/31/32/37/38/40/43/62/67/68/70/71/88/98/111) gained a tailored snapshot-or-interaction-test acceptance criterion. A new test, the_ui_test_convention_is_documented_where_agents_read_it, fails the build if either half of the convention is deleted. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p sub-ui, all clean.
<!-- SECTION:FINAL_SUMMARY:END -->
