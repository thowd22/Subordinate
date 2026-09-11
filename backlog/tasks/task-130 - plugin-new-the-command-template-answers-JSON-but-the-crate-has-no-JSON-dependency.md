---
id: TASK-130
title: >-
  plugin new: the command template answers JSON but the crate has no JSON
  dependency
status: Done
assignee:
  - '@opus-task-130'
created_date: '2026-09-10 21:59'
updated_date: '2026-09-11 00:42'
labels:
  - plugins
  - cli
milestone: m-6
dependencies:
  - TASK-102
priority: low
ordinal: 150000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the TASK-102 runbook run. A command plugin's run returns a JSON document, and the scaffolded src/lib.rs builds its answer with format! over hand-written braces because the generated Cargo.toml depends only on subordinate-sdk. The first thing the agent did after reading the template was add serde_json to Cargo.toml. Either the scaffold should ship that dependency, or the SDK should offer the answer type so a plugin never hand-rolls JSON.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A scaffolded command plugin can build its answer without the author adding a dependency
- [x] #2 The CLAUDE.md the scaffold writes shows the way it intends
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Re-export serde_json (and its json! macro) from subordinate-sdk, which already depends on it, so a plugin never adds a JSON dependency of its own.
2. Rewrite the command world template's answer to use subordinate_sdk::json!/serde_json instead of hand-written braces in format!; do the same for the mcp-tools template.
3. Update the scaffolded CLAUDE.md (interface and house rules) to show the intended way: build the answer with the SDK's re-exported serde_json, do not add a dependency.
4. Update the SDK crate docs' command examples to match.
5. Tests: scaffold unit/integration assertions that the template and guide say so and the Cargo.toml still has one dependency; an SDK test that json! and serde_json are reachable through the SDK.
6. Verify with cargo fmt --check, clippy -D warnings, and the touched crates' tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The SDK already depended on serde_json — it is in the public surface, since run_json/query_json take and return serde_json types — so the fix was to re-export it rather than to grow the scaffold's dependency list. subordinate-sdk now re-exports `serde_json` and its `json!` macro at the crate root; the command and mcp-tools templates build their answers with `json!({ ... }).to_string()` (the command template also parses `args` with `serde_json::from_str`) instead of format! over hand-written braces, and the scaffolded Cargo.toml keeps its single dependency with a comment saying why.

The scaffolded CLAUDE.md now says the intended way in two places: the command and mcp-tools interface sections show `subordinate_sdk::json!` / `subordinate_sdk::serde_json`, and a house rule states that Cargo.toml has one dependency and wants no more. The SDK's own crate docs were updated to match.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p subordinate-sdk -p subordinate-cli passes, including two new tests (scaffold::tests::the_command_scaffold_answers_json_with_the_sdks_own_and_adds_no_dependency, scaffold::tests::a_scaffolded_crate_depends_on_the_sdk_and_nothing_else) and an SDK test that json!/serde_json are reachable through the SDK. AC #1 was proven end to end by running the opt-in build test with SUBORDINATE_SCAFFOLD_BUILD=1 and SUBORDINATE_SDK_PATH pointing at this checkout: all four templated worlds compiled to wasm32-wasip2 and installed with no edits (tests/scaffold.rs a_scaffold_builds_and_installs_with_no_edits, 58s).

Left alone deliberately: the first-party plugins plugins/cut-silence and plugins/otio keep their own serde_json dependency. They are workspace crates rather than scaffolds, and changing them is outside this task's criteria.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
subordinate-sdk now re-exports serde_json and its json! macro, and the plugin new templates use them: a scaffolded command plugin builds its JSON answer and reads its arguments with what the SDK already carries, so its Cargo.toml keeps the one dependency it is generated with. The scaffolded CLAUDE.md says so in the interface section and in a house rule, and the SDK's own docs match. Verified with cargo fmt --check, clippy -D warnings, the sdk and cli test suites (three new tests), and the opt-in scaffold build test, which compiled and installed all four worlds for wasm32-wasip2 with no edits.
<!-- SECTION:FINAL_SUMMARY:END -->
