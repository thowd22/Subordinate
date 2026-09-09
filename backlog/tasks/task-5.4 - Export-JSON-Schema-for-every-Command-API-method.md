---
id: TASK-5.4
title: Export JSON Schema for every Command API method
status: Done
assignee:
  - '@opus-task-5.4'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 16:45'
labels:
  - core
  - mcp
milestone: m-0
dependencies:
  - TASK-5.3
references:
  - docs/PLAN.md
parent_task_id: TASK-5
priority: medium
ordinal: 33000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The MCP bridge and plugin SDK generate their tool and binding definitions from this schema, so it must be produced by the code, not hand-written.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 subordinate-cli schema dumps a JSON document listing every method with params and result schemas (schemars)
- [x] #2 A CI check fails if the committed docs/schema/command-api.json differs from the generated one
- [x] #3 Each method carries a one-sentence description used verbatim as its MCP tool description
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-edit: add schemars dep; require JsonSchema + a one-sentence DESCRIPTION const on the Command trait; derive JsonSchema on every command params type and on ChangeEvent.
2. sub-edit: CommandRegistry stores per-kind metadata (description + schema fn); expose registry.description(kind) and registry.params_schema(kind, generator).
3. sub-command: every dispatcher method carries a description and params/result schema. Extend Dispatcher::add/register to take them; new sub_command::schema module builds the whole document (methods array + shared $defs) via one schemars SchemaGenerator, plus schema_text().
4. Commit docs/schema/command-api.json and add a committed_command_api_schema_is_up_to_date test (SUB_UPDATE_SCHEMA=1 regenerates), which CI runs via cargo test --workspace. Document it in docs/schema/README.md.
5. subordinate-cli: add a 'schema' subcommand printing the document (--compact for one line); tests for the subcommand parse and output.
6. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test for sub-edit, sub-command, subordinate-cli, sub-model.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
sub-edit: the Command trait now requires schemars::JsonSchema and carries a DESCRIPTION const (defaulted to "" so test doubles need not set one). CommandRegistry stores per-kind metadata alongside the decoder and exposes description(kind) and params_schema(kind, generator); AnyCommand gained description_erased. Every built-in command params type and ChangeEvent now derive JsonSchema.

Gap found and fixed: register_builtin never called clip::register, so clip.add/remove/move/trim_in/trim_out/split/ripple_delete and edit.restore_track_items were registered nowhere and the Command API served none of them (contradicting the dispatch module docs). One line added to register_builtin; the builtin registry now holds 33 kinds.

sub-command: dispatcher method table entries carry a description and params/result schemers. Query and session results are now typed structs (ProjectResult, RevisionResult, UndoResult, BeginGroupResult, CommitGroupResult, AbortGroupResult, ListMethodsResult) so each has a real result schema; NoParams and BeginGroupParams became public. Dispatcher::register gained a description and P/R type parameters, so a plugin-contributed method is exported like a built-in one. New sub_command::schema builds the whole document from one SchemaGenerator (shared $defs), sorts every object key and appends a newline, matching sub_model::json's byte-stability guard.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-edit -p sub-command -p sub-model -p subordinate-cli all pass (including the 5 schema unit tests, the 3 subordinate-cli schema integration tests, and every doctest).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The Command API now exports its own JSON Schema. docs/schema/command-api.json lists all 44 served methods -- 33 commands, 9 queries, 2 session methods -- each with its kind, a one-sentence description and the schemars-generated schema of its params and result, over a shared $defs. It is produced by the code: a command's schema comes from the serde type the engine decodes and its description from Command::DESCRIPTION, so the document cannot describe a method this build does not serve. subordinate-cli schema prints it (--compact for one line). Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test across sub-edit, sub-command, sub-model and subordinate-cli, all clean; the committed_schema_is_up_to_date test (run by CI's cargo test --workspace) fails on any drift, and a subordinate-cli integration test asserts what the CLI prints equals the committed file.
<!-- SECTION:FINAL_SUMMARY:END -->
