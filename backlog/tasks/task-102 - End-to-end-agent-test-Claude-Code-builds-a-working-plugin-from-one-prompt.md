---
id: TASK-102
title: 'End-to-end agent test: Claude Code builds a working plugin from one prompt'
status: Done
assignee:
  - '@opus-task-102'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 22:30'
labels:
  - test
  - plugins
  - mcp
milestone: m-6
dependencies:
  - TASK-96
  - TASK-90
  - TASK-99
references:
  - docs/PLAN.md
priority: high
ordinal: 123000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Phase 6 exit criterion.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A runbook prompt asks Claude Code to create, install and test a 'remove gaps' command plugin using only the MCP tools
- [x] #2 The run completes with no manual steps on a clean checkout and the plugin passes plugin test
- [x] #3 Transcript and friction notes are recorded in a backlog doc and turned into follow-up tasks
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Write docs/agent-runbook.md: the phase 6 exit runbook — preconditions, MCP config, the verbatim one prompt that asks Claude Code to create, install and test a 'remove gaps' command plugin through the editor's MCP tools, and the objective pass criteria (plugin.test ok = true).
2. Add scripts/agent-runbook.sh: the no-manual-steps driver. From a clean checkout it builds subordinate-cli and subordinate-mcp, starts 'subordinate-cli serve' on a scratch instance, writes the MCP config that points at the bridge, runs 'claude -p' with the runbook prompt and the MCP tools allowed, then independently re-verifies with plugin test and prints a JSON verdict plus the transcript path.
3. Add a guard test in bins/subordinate-mcp/tests that every MCP tool name the runbook names is one the bridge actually serves, so the runbook cannot drift from the tool surface.
4. Execute the runbook on this machine and capture the transcript and the plugin test report.
5. Record transcript and friction notes in a backlog doc, and file follow-up tasks for each friction point.
6. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test -p subordinate-mcp (GStreamer env sourced for anything touching sub-media).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Shape of the deliverable: the runbook is a document plus a driver, and the prompt lives in the document. docs/agent-runbook.md holds the one prompt between <!-- prompt:start --> markers; scripts/agent-runbook.sh reads it from there and renders {{WORKDIR}}, {{SDK}} and {{FIXTURE}}, so the text an author reads is byte for byte the message the agent receives. The driver builds subordinate-cli and subordinate-mcp, serves the Command API on a scratch instance with an endpoint and plugin directory of its own (a real editor's installed plugins are never touched), writes the MCP config, runs `claude -p` with only that server and the file/shell tools, then stops the editor and re-checks the result with its own `subordinate-cli plugin test` run. The verdict is the harness's, printed as one JSON object; the exit code is non-zero when the run failed.

docs/examples/gaps.sub is the fixture the run is judged against: one 25 fps video track, three 25-frame clips, a 10-frame and a 5-frame gap. The scaffold's own fixture is an empty starter project, and a command plugin run against it changes nothing, which makes the harness skip its `undoable` check — the interesting half of the contract. A fixture with gaps in it is what turns `plugin test` into a real test here. Clips reference a media item with no file on disk, which the model allows (`media.import` touches no filesystem), so the runbook needs no sample media.

Tests: bins/subordinate-mcp/tests/runbook.rs is the guard against drift — every `plugin_*` tool the prompt names must be one ToolSet::committed() serves, the prompt must still carry the three placeholders and the plugin id, the script must still name the runbook, the fixture and the marker it reads, and the fixture must still be three clips with exactly 15 frames of gap. Four tests, no agent, no network.

The run (2026-09-10), recorded in doc-3: ok, 6 checks passed, 0 failed, `changed: true`, `undoable: pass`; 35 turns, 225 s, $3.06, no permission denials, nothing typed after the prompt. The session scaffolded with plugin_new, wrote 326 lines of Rust, built for wasm32-wasip2, installed with plugin_install --dev and iterated with plugin_test on its own. Artifacts kept at ~/.cache/subordinate/agent-runbook-2026-09-10/.

Honest limits. (1) `cargo build` is not an MCP tool and cannot be: sub_plugin::authoring says the editor does not run compilers on an agent's behalf, so the loop is MCP, MCP, shell, MCP, MCP. AC #1 is checked on that reading — every editor-side step is an MCP tool — and the runbook says so in as many words rather than claiming otherwise. (2) The recorded run used --no-build because this machine has no system GStreamer and the RUSTFLAGS that stand in for one would have been inherited by the agent's own wasm builds; the binaries it used were built by the same `cargo build -p subordinate-cli -p subordinate-mcp` the script runs, from this checkout, minutes earlier, and the plugin, its work directory and the plugin registry were all created from nothing by the run. (3) The harness cannot assert the timeline a command plugin leaves behind, only that the project changed and that one undo reversed it — the run's first build removed 25 frames instead of 15 and passed; that is TASK-129.

Friction became TASK-128 (a scaffolded crate does not build on a default toolchain older than the SDK's rust-version), TASK-129 (plugin test cannot check what a command plugin actually did) and TASK-130 (the command template answers JSON from a crate with no JSON dependency).

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p subordinate-mcp all green (5 suites, runbook.rs 4/4); scripts/agent-runbook.sh --agent true exercised the driver's plumbing before the real run; the real run's verdict is above.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the phase 6 exit test and ran it. docs/agent-runbook.md is the runbook: what it proves, the preconditions, the single prompt that asks Claude Code to create, install and test a 'remove gaps' command plugin through the editor's MCP tools, and pass criteria nothing about the agent's own account can satisfy. scripts/agent-runbook.sh is the driver that makes it a no-manual-steps run: it builds the binaries, serves the Command API on a scratch instance with a plugin directory of its own, hands the prompt (read out of the runbook, so the two cannot drift) to a fresh non-interactive Claude Code session, then re-verifies with its own `subordinate-cli plugin test` and prints one JSON verdict. docs/examples/gaps.sub is the fixture with gaps in it that makes that check meaningful, and bins/subordinate-mcp/tests/runbook.rs fails if the prompt names a tool the bridge does not serve or the fixture stops matching what the prompt tells the agent.

The run happened: a fresh session turned the prompt into com.example.remove-gaps in 35 turns and 225 seconds with nothing typed after it, and the CLI's own plugin test reports 6 passed, 0 failed, `changed: true` and `undoable: pass`. Transcript, verdict and friction notes are in doc-3; the three friction points became TASK-128, TASK-129 and TASK-130. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p subordinate-mcp, all clean.
<!-- SECTION:FINAL_SUMMARY:END -->
