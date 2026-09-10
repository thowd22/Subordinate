---
id: TASK-99
title: 'First-party plugin: cut silence (analyzer + command)'
status: Done
assignee:
  - '@opus-task-99'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 18:58'
labels:
  - plugins
  - first-party
milestone: m-6
dependencies:
  - TASK-79
  - TASK-80
  - TASK-91
references:
  - docs/PLAN.md
priority: high
ordinal: 120000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The canonical agent-built plugin and the phase 6 exit test target.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Analyzer detects silent ranges below a threshold; command removes them from selected clips with ripple in one undo group
- [x] #2 Ships in plugins/ built by CI and installable via plugin install
- [x] #3 Its CLAUDE.md is the reference for the scaffold template
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add plugins/cut-silence: one WASM component in its own workspace, like plugins/gain.
2. Give it a local WIT world (wit/cut-silence.wit) that imports subordinate:plugin/command-api@0.1.0 and exports both the command world's run() and the analyzer world's analyze(), so the single plugin.wasm instantiates under the host's command and analyzer bindings alike; it imports no analysis-host so the command-world linker also satisfies it.
3. src/wav.rs: minimal RIFF/WAVE PCM reader (16/24/32-bit int, 32-bit float) for the media the analyzer is pointed at, reached at /project/<relative path> under a fs_read = [\$PROJECT] grant.
4. src/silence.rs: windowed RMS detector - threshold in dBFS, minimum silent duration, padding kept at each edge - producing silent spans in exact sample counts (no float timeline math).
5. src/plan.rs: the edit planner. Intersect each silence range (media time) with each clip's source range, map exactly onto sequence time with i128 arithmetic (ceil the start, floor the end so nothing audible is cut), and order the cuts last-first per track.
6. src/lib.rs: analyze() = query project.get for the media path, decode, detect, return analysis-result ranges/markers labelled 'silence'; run() = read clips through command-api's typed accessors, plan, then edit.begin_group + clip.split/clip.ripple_delete + edit.commit_group so the whole cut is one undo step, aborting the group on any failure.
7. plugin.toml declaring worlds = [analyzer, command] and the fs_read capability; CLAUDE.md written as the reference the scaffold template points at.
8. CI: build the component and run its host-triple tests, next to the existing gain step.
9. Verify: cargo fmt/clippy/test in the plugin workspace and cargo build --release --target wasm32-wasip2; workspace fmt/clippy/test for the CI change.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design: a plugin is one component and a component has one world, but this task needs an export from two (analyzer's analyze, command's run). plugins/cut-silence/wit/cut-silence.wit therefore declares a world of its own that is the union of the two — the host's own export signatures and types, with command-api imported once — so the single plugin.wasm instantiates against the host's analyzer bindings and its command bindings alike, and plugin.toml declares worlds = [analyzer, command] honestly. It deliberately does not import analysis-host: a component only instantiates when every import it declares is in the linker, and the host's command world links command-api alone, so importing it would make the component loadable as an analyzer but not as a command. The cost — no progress reports, no cancellation before analyze returns — is affordable because the analyzer reads one file once, and both the WIT and CLAUDE.md say so and say when to split an analyzer into its own component instead.

Structure: src/wav.rs is a small RIFF/WAVE reader (16/24/32-bit int, 32/64-bit float), src/silence.rs a windowed-RMS detector working in whole audio frames, src/plan.rs the timeline arithmetic (media time to sequence ticks in exact i128, start rounded up and end rounded down so a cut never eats audible material, cuts ordered last-first per track). All three are WIT-free and unit-tested on the host triple; src/lib.rs is glue. run() reads clips through the typed command-api accessors, plans, then applies clip.split/clip.split/clip.ripple_delete per cut, re-reading the track between steps because a split gives the tail a fresh identity. Two things the host taught the implementation: (1) the host already opens an undo group around a plugin run, and groups do not nest, so the plugin opens one only when history.get says none is open and closes only what it opened; (2) the host places a clip in whatever rate its arithmetic landed on (a clip after a split is placed at the media sample rate, not the sequence frame rate), so every timeline span is rescaled to the sequence rate before it is compared with anything.

Verification. plugins/cut-silence: cargo build --release --target wasm32-wasip2, cargo test (18 unit + 3 integration + 1 doc test, all pass), cargo fmt --all --check, cargo clippy --all-targets clean. Workspace: cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test -p subordinate-cli all pass. End to end on this machine: subordinate-cli plugin install <component> --dev loaded it (status ok, generation 1), and subordinate-cli plugin test com.subordinate.cut-silence reports 9 passed, 0 failed, 0 skipped over both declared worlds — the component instantiates under the analyzer and the command bindings, analyze returns findings, and run turns the fixture's one clip into three (two splits and two ripple deletes, 75 frames of 25 fps removed) which the harness's undoable check reverses with a single undo. That single undo is the evidence for the one-undo-group criterion. The GitHub Actions run of the new CI step is not verifiable from this environment; the step mirrors the existing plugins/gain one exactly.

Known limits: analyze reads the media through the fs_read grant on $PROJECT and decodes linear PCM .wav only, so other containers are reported, not decoded. A sandbox with no filesystem grant — which is what the plugin test harness is — makes analyze answer an analysis with no ranges and status = unread plus the reason, rather than failing the job; an analysis is a finding, and that is one. fixture/fixture.sub therefore carries a silence analysis already stored, as an analyzer run would have left it, so the harness exercises the cut itself.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added plugins/cut-silence, the first-party plugin that finds silence and cuts it out: one WASM component in its own workspace exporting both the analyzer world's analyze and the command world's run, through a union world of its own (wit/cut-silence.wit). analyze reads the media item's file through the fs_read grant on $PROJECT, decodes PCM .wav and reports silent spans in media time; run reads those stored findings back, maps them onto the selected clips in exact rational arithmetic and removes them with clip.split and clip.ripple_delete inside one undo group. Ships with plugin.toml, a fixture project, and a CLAUDE.md that the subordinate-cli plugin new template now names as its worked example; CI builds the component and runs its tests beside plugins/gain. Verified with the plugin's own cargo test (22 tests), workspace fmt/clippy/subordinate-cli tests, and a real subordinate-cli plugin install plus plugin test run: 9 checks pass over both worlds, the run turns one clip into three and one undo puts it back.
<!-- SECTION:FINAL_SUMMARY:END -->
