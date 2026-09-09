---
id: TASK-6
title: 'Build subordinate-cli: load, save and inspect a project headlessly'
status: Done
assignee:
  - '@opus-task-6'
created_date: '2026-09-08 20:53'
updated_date: '2026-09-09 20:09'
labels:
  - cli
milestone: m-0
dependencies:
  - TASK-5
references:
  - docs/PLAN.md
priority: medium
ordinal: 6000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A headless CLI is the CI smoke test and the fallback the MCP bridge launches when no GUI is running (PLAN.md §7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 subordinate-cli new, open, save and inspect commands work on a sample project
- [x] #2 subordinate-cli serve exposes the Command API socket without a GUI
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Extend the subordinate-cli argument parser with new, open, save, inspect and serve subcommands alongside the existing version/diag/schema ones, keeping the hand-rolled parser and its USAGE text in step.
2. Add a project module: 'new' writes a fresh .sub file (a Main sequence with V1/A1 added through sub-edit Commands on an Engine, never by hand-mutating the model), 'open' loads through sub_model::json::from_json_with_report and reports schema version, migrations and offline media, 'save' round-trips a file to deterministic JSON at the same or a new path, 'inspect' prints the project structure (sequences, settings, tracks, clips, media, bins) with every time as an exact RationalTime plus timecode, never a float.
3. Add a serve module: bind sub_command::transport::Server on the per-user endpoint (or --directory for tests), optionally loading a project first, print the endpoint/lock JSON on stdout as a readiness line, then serve until stdin reaches EOF and shut the server down cleanly.
4. Errors are SubError printed as JSON on stderr with a failure exit code; all output is JSON with --compact/--pretty like the existing subcommands.
5. Tests: unit tests for the parser; integration tests bins/subordinate-cli/tests/project.rs (new -> inspect -> save -> open round trip on a sample project) and serve.rs (spawn serve, read the readiness line, connect a sub_command Client from the test process, call project.get and system.list_methods, close stdin and check clean exit and lock-file removal).
6. Verify with cargo fmt --check, clippy pedantic -D warnings and cargo test for subordinate-cli (GStreamer env sourced, since the CLI depends on sub-media).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in bins/subordinate-cli: src/project.rs (new/open/save/inspect) and src/serve.rs (the Command API without a GUI), wired into the existing hand-rolled parser in src/main.rs. sub-time was added as a dependency so inspect can report exact rates and timecodes.

Decisions:
- 'new' builds its starter Main sequence plus V1/A1 through sub_edit CreateSequence and AddTrack on a real Engine, never by mutating Project directly, so the file the CLI writes is one the editor could have produced. It refuses to overwrite an existing file without --force (core.invalid_argument).
- 'inspect' reports every time as its exact RationalTime value and rate plus the seconds fraction as integer strings and a timecode; no float appears anywhere in the report. Offline media is computed on a clone, because an inspection must not edit.
- 'save' loads (which migrates an older file) and rewrites deterministic JSON, so it doubles as the headless migrate-in-place command; 'changed' says whether the bytes moved.
- 'serve' binds sub_command::transport::Server on Endpoint::for_instance (or --directory for tests), prints one readiness JSON line naming transport, address, lock file and pid before waiting, then serves until stdin reaches EOF and shuts the server down, removing the lock file. --project loads a project first; without one it serves an empty 'Untitled'.
- Every subcommand prints JSON (indented, --compact for one line) and reports failures as a JSON SubError on stderr with a non-zero exit.

Validation (Linux, local GStreamer prefix sourced): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p subordinate-cli 33 tests pass, including tests/project.rs (new -> open -> inspect -> save round trip, byte-identical resave, overwrite refusal, bad-file and missing-file errors) and tests/serve.rs (a second process reads the readiness line, finds the endpoint from the directory and lock file, calls project.get, system.list_methods, track.add and history.get over the socket, then closing stdin stops the server and removes the lock file).

docs/DEVELOPMENT.md gained a 'Working on a project headlessly' section covering the five subcommands and the readiness-line protocol.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
subordinate-cli now loads, saves and inspects projects headlessly and serves the Command API without a GUI: new, open, save, inspect and serve, all printing JSON (and JSON SubErrors on stderr). new builds its starter sequence and tracks through undoable sub-edit commands on an Engine; inspect reports exact RationalTime values, rates and timecodes with no floats; serve binds the per-user endpoint, prints a readiness line naming the address and lock file, and serves until stdin closes. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test -p subordinate-cli (33 tests), including an end-to-end new/open/inspect/save round trip and a second process driving the served socket with project.get, system.list_methods, track.add and history.get.
<!-- SECTION:FINAL_SUMMARY:END -->
