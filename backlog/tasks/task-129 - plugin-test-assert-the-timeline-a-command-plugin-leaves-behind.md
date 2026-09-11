---
id: TASK-129
title: 'plugin test: assert the timeline a command plugin leaves behind'
status: Done
assignee:
  - '@opus-task-129'
created_date: '2026-09-10 21:58'
updated_date: '2026-09-11 01:18'
labels:
  - plugins
  - test
milestone: m-6
dependencies:
  - TASK-102
priority: medium
ordinal: 149000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the TASK-102 runbook run. The harness proves a command plugin changed the project and that one undo reversed it, but its project_state check carries only counts (clips, tracks, markers, revision), so a plugin that moved every clip to the wrong place passes exactly like one that got it right. In the recorded run the agent's first build removed 25 frames instead of 15 and the harness could not tell; only the plugin's own answer revealed it. A fixture needs a way to state the timeline it expects afterwards so plugin test checks the result rather than the fact of a change.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A fixture can declare the track layout expected after a command plugin runs, and plugin test reports a failed check when the result differs
- [x] #2 The expectation is exact RationalTime, not seconds
- [x] #3 A fixture that declares no expectation behaves as it does today
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-plugin/src/expect.rs: a TimelineExpectation type (sequences -> tracks -> clips) deserialised from a sidecar JSON file, with clip start/duration as exact serde RationalTime (value + rate numerator/denominator), never seconds.
2. Sidecar convention: for fixture <name>.sub the expectation is <name>.expect.json next to it; absent means no expectation.
3. Harness::with_expectation(...); the command world's project-state assertion gains a 'timeline_matches' check: pass when the timeline after the run matches, fail listing every mismatch, skip when no expectation was declared (so today's behaviour is unchanged).
4. subordinate-cli plugin_test: look for the sidecar beside the chosen fixture, load it (invalid JSON is core.invalid_argument), hand it to the harness and report its path in the JSON output.
5. Unit tests for expectation matching/mismatch and RationalTime exactness (frames at 24 vs 48), a harness/CLI test proving a mismatch fails the run and that a fixture with no sidecar behaves as before. Run fmt, clippy -D warnings, tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-plugin/src/expect.rs: TimelineExpectation (sequences -> tracks -> clips), read from a sidecar beside the fixture (fixture.sub -> fixture.expect.json, TimelineExpectation::beside). Clip start and duration are serde RationalTime — an integer value at an exact rational rate — and are compared with RationalTime's own equality, which is exact across timebases (15@24 == 30@48) and never seconds or floats. Sequence/track names, track kind and clip names are optional; matching is positional.

Harness::with_expectation feeds it to the command world, which now pushes a 'timeline_matches' check beside project_state: pass when the timeline after the run is the declared one, fail carrying a 'mismatches' array naming each path (sequences[0].tracks[0].clips[1].start) with both times printed exactly (15@24/1 vs 25@24/1), skip when no expectation was declared — so a fixture without a sidecar runs exactly as before (project_state, undoable and ok unchanged).

subordinate-cli plugin test reads the sidecar beside whichever fixture it chose, before loading the plugin; a malformed one is core.invalid_argument naming its path, and the report carries 'expectation' (the sidecar path, or null). The scaffolded command-plugin CLAUDE.md now documents the sidecar with an example, so an agent writing a plugin knows to state what it expects.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the local GStreamer env); cargo test -p sub-plugin (158 lib tests incl. 10 new expect tests, harness integration 8/8 incl. the three new ones) and cargo test -p subordinate-cli --bin subordinate-cli (47 passed incl. the two new plugin_test ones). AC1 proven by a_timeline_that_is_not_what_the_fixture_declared_fails_the_run; AC2 by times_are_compared_as_exact_instants_across_timebases and a_fixture_that_declares_the_timeline_it_expects_has_it_asserted (same timeline stated at 24 and 48 fps); AC3 by a_fixture_that_declares_no_expectation_is_unchanged and a_fixture_with_no_expectation_reports_none.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A fixture can now declare the timeline it expects a command plugin to leave behind, in a sidecar beside it (fixture.sub -> fixture.expect.json): tracks and their clips with exact RationalTime start and duration in sequence time. The harness's command world gained a timeline_matches check that fails naming every clip in the wrong place — the class of failure the old counts-only project_state check could not see — and skips, unchanged, for a fixture that declares nothing. Verified by new unit tests in sub-plugin, three new harness integration tests over the real editor guest, two new CLI tests, and clean fmt and clippy -D warnings.
<!-- SECTION:FINAL_SUMMARY:END -->
