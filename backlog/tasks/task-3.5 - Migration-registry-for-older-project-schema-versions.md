---
id: TASK-3.5
title: Migration registry for older project schema versions
status: Done
assignee:
  - '@opus-task-3.5'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 01:22'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-3.4
references:
  - docs/PLAN.md
parent_task_id: TASK-3
priority: medium
ordinal: 23000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Old files must always open (§5.6). A registry pattern from version 1 onward avoids ad-hoc upgrade code later.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Migration trait with from_version, to_version and a function over serde_json::Value
- [x] #2 Loader applies migrations in order and records the original version in a load report
- [x] #3 A test fixture at schema_version 1 with a deliberately renamed field migrates and loads
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub-model::migrate: Migration trait (from_version, to_version, describe, migrate over serde_json::Value), MigrationRegistry keyed by from_version with a target version, FnMigration helper, LoadReport recording the original version and each applied step.
2. Add stable error code model.migration_failed; keep model.unsupported_schema_version for newer files and for gaps in the chain.
3. Rewire json::from_json through the registry; add from_json_with_report and from_json_with_registry so the loader applies migrations in order and returns the load report.
4. Add a committed fixture at schema_version 1 with a deliberately renamed field plus an integration test that migrates it through a 1 -> 2 registry and loads the project.
5. Verify: cargo fmt --check, clippy pedantic -D warnings, cargo test -p sub-model.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-model/src/migrate.rs: the Migration trait (from_version, to_version, describe, migrate over serde_json::Value on the whole file, not just the project member), FnMigration for closure steps, MigrationRegistry keyed by the version each step reads, and LoadReport { original_version, final_version, applied }. The registry stamps schema_version after each step so a step never has to; register() rejects steps that do not move forward, overshoot the target, or collide on a from_version.

json::from_json now goes text -> Value -> registry -> model, so a file is only deserialised once it is at the current version. New public entry points from_json_with_report and from_json_with_registry return the LoadReport; from_json keeps its old signature. New stable error code model.migration_failed wraps a failing step with from_version, to_version and the step description in details; a gap in the chain stays model.unsupported_schema_version with schema_version, stuck_at_version and supported_schema_version.

SCHEMA_VERSION is still 1, so MigrationRegistry::current() is legitimately empty: version 1 is the first schema and there is nothing to upgrade from yet. AC 3 is therefore proven with a committed real v1 file (crates/sub-model/tests/fixtures/project-v1-renamed-field.json, generated from the model and then edited so the project name sits under 'title') loaded through a registry targeting a hypothetical version 2 with the rename step a real bump would register — the same code path production loads take. crates/sub-model/tests/migration.rs also asserts the fixture does NOT load without its migration, and that the migrated project saves back at SCHEMA_VERSION with no 'title'.

clippy::wrong_self_convention is allowed on the Migration trait with a comment: from_version reads a field and the from/to pair is the vocabulary the AC and the schema chain use.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the scratchpad GStreamer pkg-config env for sub-media); cargo test -p sub-model 75 unit + 4 integration + 5 doc tests pass; cargo test for sub-core, sub-time, sub-edit and sub-command also pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added a migration registry to sub-model: a Migration trait over serde_json::Value keyed by from_version/to_version, a MigrationRegistry that walks the chain up to the target schema version and stamps each new version, and a LoadReport recording the file's original version and every step applied. json::from_json now migrates before deserialising and from_json_with_report / from_json_with_registry expose the report; failures use the new stable code model.migration_failed. Verified with a committed schema_version 1 fixture whose project name is stored under a renamed 'title' field: it migrates and loads through a 1 -> 2 registry, fails to load without that step, and saves back at the current SCHEMA_VERSION (cargo test -p sub-model, plus fmt --check and clippy --workspace -D warnings clean).
<!-- SECTION:FINAL_SUMMARY:END -->
