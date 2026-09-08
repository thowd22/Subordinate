---
id: TASK-11
title: >-
  Error and logging conventions: thiserror error types, tracing setup,
  structured error codes
status: Done
assignee:
  - '@opus-task-11'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-08 22:37'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-1.1
references:
  - docs/PLAN.md
priority: medium
ordinal: 15000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Errors must be machine-readable because plugins and the MCP bridge surface them to agents (§6.4). Deciding on one convention before the model and command crates are written avoids retrofitting.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A shared SubError type with a stable string code, human message and optional details map, serializable to JSON
- [x] #2 tracing is initialised in every binary with env-filter and a JSON output option
- [x] #3 docs/DEVELOPMENT.md documents the convention: when to add a code, how to wrap lower-level errors
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-core (lowest layer, no deps on other sub-* crates) holding the shared error and logging conventions; add it to the workspace members.
2. error module: ErrorCode (validated, dot-namespaced stable string, const constructors + a codes registry of the common core codes) and SubError { code, message, details: BTreeMap<String, JsonValue>, cause } with thiserror, serde Serialize/Deserialize, builder methods, wrap() for lower-level errors and a ResultExt context helper. SubResult<T> alias.
3. logging module: LogFormat { Text, Json }, LogConfig resolved from SUBORDINATE_LOG / RUST_LOG / SUBORDINATE_LOG_FORMAT, init()/try_init() over tracing-subscriber with EnvFilter and a JSON output option; failures reported as SubError.
4. Call the initialiser from all three binaries (subordinate, subordinate-cli, subordinate-mcp).
5. Document the convention in docs/DEVELOPMENT.md: when to add a code, code naming, how to wrap lower-level errors, logging setup and env vars.
6. Unit tests for code validation, JSON shape and round-trip, details, wrapping/cause chain, and log config resolution. Verify fmt, clippy -D warnings and tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-core as the lowest workspace crate (nothing of ours below it) so sub-model, sub-media, sub-command and the plugin host can all depend on the conventions without a cycle; docs/PLAN.md §4 crate list updated to match.

error module: ErrorCode is a validated dot-namespaced string (const from_static for code constants, parse() for runtime/deserialized values, which rejects malformed codes). SubError { code, message, details: BTreeMap<String, serde_json::Value>, cause } derives serde with details/cause skipped when empty, so the JSON shape is stable and compact. Lower-level errors are flattened to their whole source() chain at construction (SubError::wrap / with_cause / ResultExt::sub_context), which keeps SubError Clone + Send + serializable — important because it crosses the Command API, MCP and WASM boundaries. Nine shared core.* codes; other crates declare their own domain constants.

logging module: LogConfig::resolve() reads SUBORDINATE_LOG (falling back to RUST_LOG) and SUBORDINATE_LOG_FORMAT (text|json) through an injectable variable lookup so precedence is unit-testable; init() installs a tracing-subscriber fmt layer with EnvFilter writing to stderr (stdout carries the MCP protocol and CLI output), JSON via .json().flatten_event(true). Failures are SubError (core.invalid_argument, core.logging_init). All three binaries call sub_core::logging::init("info") first and return ExitCode::FAILURE with the rendered SubError if it fails.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the scratchpad GStreamer env for sub-media); cargo test --workspace passes — 15 sub-core unit tests, 2 doctests, and 4 integration tests in bins/subordinate-cli/tests/logging.rs that spawn the real binary and assert text logs land on stderr only, that the env-filter suppresses info at warn, that SUBORDINATE_LOG_FORMAT=json yields one parseable JSON object per event with level/target/message/version fields, and that a bad format exits non-zero printing [core.invalid_argument].
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the sub-core crate holding both conventions: SubError (stable ErrorCode, human message, optional details map, flattened cause chain) with a stable, round-tripping JSON shape, and a tracing setup with an env-filter and a text/JSON output option that all three binaries now install on startup. docs/DEVELOPMENT.md now documents the convention — code naming and when to add one, how to wrap lower-level errors at crate boundaries, and the logging env vars. Verified with cargo fmt --check, cargo clippy --workspace --all-targets -D warnings and cargo test --workspace (15 unit + 2 doc tests in sub-core, plus 4 integration tests that run subordinate-cli and assert the stderr text and JSON log output and the failure path).
<!-- SECTION:FINAL_SUMMARY:END -->
