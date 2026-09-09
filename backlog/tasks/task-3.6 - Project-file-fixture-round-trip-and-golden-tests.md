---
id: TASK-3.6
title: Project file fixture round-trip and golden tests
status: Done
assignee:
  - '@opus-task-3.6'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 01:37'
labels:
  - core
  - test
milestone: m-0
dependencies:
  - TASK-3.5
references:
  - docs/PLAN.md
parent_task_id: TASK-3
priority: medium
ordinal: 24000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Byte-identical round-trips prove the format is stable and give later refactors a regression net.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A committed sample project JSON with two sequences, three tracks each, clips, a crossfade, markers and two bins loads and saves byte-identically
- [x] #2 Golden test fails with a readable diff when output changes
- [x] #3 Sidecar directory naming (project.sub.d/) and its gitignore entry are documented in docs/PLAN.md §5.6 and DEVELOPMENT.md
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a committed sample project fixture crates/sub-model/tests/fixtures/sample-project.sub: two sequences, three tracks each, clips, a crossfade, markers and two bins, with hard-coded UUIDv7 identifiers so the bytes are reproducible.
2. Add crates/sub-model/tests/golden.rs: a deterministic builder for that project, a load->save byte-identity round-trip test, and a golden test comparing builder output against the committed bytes.
3. On mismatch print a readable unified-style line diff naming the first differing lines plus context, and tell the reader to regenerate with SUB_UPDATE_GOLDEN=1.
4. Document the sidecar directory naming (project.sub.d/) and its gitignore entry in docs/PLAN.md 5.6 and docs/DEVELOPMENT.md.
5. Verify with cargo fmt --check, clippy -D warnings and cargo test -p sub-model.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-model/tests/fixtures/sample-project.sub (617 lines): two sequences (Main at UHD 23.976, Titles at HD 25), three tracks each, six clips, one crossfade, three sequence/clip markers, three media items with hash, stream info and all three ProxyState variants, and two child bins under the root bin. All identifiers are fixed UUIDv7 strings so the bytes are reproducible.

crates/sub-model/tests/golden.rs holds five tests: the committed fixture loads and saves byte-identically (twice, to rule out a lucky normalisation); the in-code builder serialises to exactly the committed bytes; the fixture still contains what it promises (two sequences, three tracks each, one crossfade, markers, two bins, every media item filed in a bin); and two tests over the diff helper itself. On mismatch the assertion prints the first differing line number, three lines of context, the -/+ pair, both line counts and the regeneration hint. Verified against a real failure by editing the fixture: the message pointed at line 137 with - "name": "Maim" / + "name": "Main".

SUB_UPDATE_GOLDEN=1 cargo test -p sub-model --test golden rewrites the fixture; the two fixture-reading tests skip in that mode because tests run in parallel with the writer.

Docs: docs/PLAN.md §5.6 now spells out the sidecar naming rule (name.sub owns name.sub.d/), that it is derived data, and the *.sub.d/ .gitignore entry (already present in the repo .gitignore). docs/DEVELOPMENT.md §Project files says the same with the gitignore snippet, and documents the golden fixture and how to regenerate it.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (GStreamer sysroot sourced); cargo test -p sub-model all green (5 golden + 4 migration + unit + doctests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Committed a realistic sample project file (two sequences, three tracks each, clips, a crossfade, markers, two bins) and a golden test suite that proves it loads and saves byte-identically and that the in-code builder still produces exactly those bytes, failing with a first-differing-line diff plus regeneration hint when the format changes. Sidecar directory naming (name.sub.d/) and its *.sub.d/ gitignore entry are now documented in docs/PLAN.md §5.6 and docs/DEVELOPMENT.md. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-model, all clean, plus a deliberate fixture edit to see the diff output.
<!-- SECTION:FINAL_SUMMARY:END -->
