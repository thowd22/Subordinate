---
id: TASK-79
title: WIT analyzer world integrated with the job queue
status: Done
assignee:
  - '@opus-task-79'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 01:15'
labels:
  - plugins
milestone: m-6
dependencies:
  - TASK-75
  - TASK-25
references:
  - docs/PLAN.md
priority: medium
ordinal: 100000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Background analysis such as silence detection or transcripts (§6.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 analyzer world exports analyze(media-id, options) that can stream progress and returns markers, metadata or ranges
- [x] #2 Host runs analyzers as cancellable jobs and stores results on the media item
- [x] #3 Results can be turned into markers via a command
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. WIT: add an `analysis` interface (analysis-marker, analysis-range, analysis-result), an `analysis-host` import (report-progress, is-cancelled) and a `world analyzer` exporting analyze(media-id, options) -> result<analysis-result, error> to wit/subordinate-plugin.wit.
2. sub-model: new analysis.rs with MediaAnalysis { analyzer, markers, ranges, metadata } and AnalysisRange { id, label, range }; MediaItem gains analyses (serde default, skipped when empty so v1 files and the golden stay valid); regenerate docs/schema/project-v1.schema.json.
3. sub-plugin: generate the analyzer world bindings beside the command world (reusing the existing types/command-api modules), convert a WIT analysis-result into a MediaAnalysis (host assigns marker ids, parses metadata JSON, rejects bad rationals/ranges).
4. sub-plugin: an Analyzer trait plus a job runner that submits an analysis onto sub_core::jobs::JobService, forwards progress and cooperative cancellation through JobContext, and on success stores the result on the media item by applying a command through the EngineHandle.
5. sub-edit: commands media.set_analysis, media.remove_analysis, media.replace_analyses (the shared inverse) and marker.from_analysis, which copies a stored analysis's markers and ranges onto a clip in source time; register them and regenerate docs/schema/command-api.json.
6. Tests: WIT world links; conversion round-trips; job cancellation and progress; command apply/undo; schema and golden freshness. Verify fmt, clippy pedantic and cargo test for the touched crates.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
WIT: wit/subordinate-plugin.wit gained the analysis interface (analysis-marker, analysis-range, analysis-result, all in media time), the analysis-host import (report-progress, is-cancelled) and world analyzer, which imports command-api plus analysis-host and exports analyze(media-id, options) -> result<analysis-result, error>. Findings carry no identifiers: a sandboxed plugin has no entropy the host would trust, so the host mints the MarkerIds when it stores them, which is what makes the later marker command replayable.

Model: sub_model::Analysis { analyzer, markers, ranges, metadata } and AnalysisRange { id, label, range } in a new analysis.rs; MediaItem gained analyses, #[serde(default, skip_serializing_if = "Vec::is_empty")], so the golden project fixture is byte-identical and a file written before analyses existed still loads (covered by a_media_item_written_before_analyses_existed_still_loads). SCHEMA_VERSION stays 1 and no migration was registered, matching how TASK-4.3 added Track::muted/locked. Regenerated docs/schema/project-v1.schema.json and docs/schema/command-api.json with SUB_UPDATE_SCHEMA=1.

Commands (sub-edit): media.set_analysis (upsert by analyzer name), media.remove_analysis and media.replace_analyses (the shared inverse, also its own inverse), marker.from_analysis and marker.replace_on_clip (its inverse). marker.from_analysis copies a stored analysis's markers and ranges onto a clip: analysis times are media times and clip markers are in source time, so nothing is converted or rounded; findings outside the clip's source_range are skipped, and a finding already on the clip is skipped, which makes the command idempotent. New codes edit.analysis_not_found and edit.duplicate_analysis. The Command API and the MCP bridge pick the five methods up automatically (50 methods now; the subordinate-mcp tool-count assertions were updated).

Host (sub-plugin): a second bindgen! expansion generates the analyzer world with 'with' pointing types and command-api at the ones the command world already generated, so command-api is one type on both sides. convert::analysis turns WIT findings into an Analysis, validating rationals and ranges and parsing metadata JSON (plugin.invalid_metadata is new). analyzer.rs holds the Analyzer trait (the seam a wasmtime instance sits behind, TASK-84), AnalysisContext (the host side of analysis-host) and AnalysisJobs, which submits a run to sub_core::jobs::JobService, forwards progress, and on success applies media.set_analysis through the EngineHandle. A cancelled run stores nothing even if the analyzer returned findings anyway.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test --workspace exited 0 (no failures anywhere), plus targeted runs of sub-model (83 lib tests), sub-edit (60 lib tests incl. 8 new analysis-command tests), sub-plugin (8 job tests, 8 host-interface tests), sub-command, subordinate-cli and subordinate-mcp.

AC evidence. AC1: the analyzer world is in wit/subordinate-plugin.wit and the_analyzer_world_links_with_every_import_satisfied builds a wasmtime Linker over it with no import left unsatisfied; the_analysis_host_import_is_the_jobs_progress_and_cancel_channel exercises report-progress and is-cancelled; analysis_findings_become_a_stored_analysis_with_host_assigned_ids covers markers, ranges and metadata crossing in. AC2: tests/analyzer_jobs.rs runs a real JobService and a real Engine -- findings_land_on_the_media_item_and_can_be_undone (progress events reach the queue, findings land on the MediaItem, undo removes them), cancelling_a_running_analysis_stores_nothing, a_run_cancelled_after_it_finished_still_stores_nothing, a_queued_analysis_that_is_cancelled_never_runs, a_second_run_of_the_same_analyzer_replaces_the_first, plus the failure paths. AC3: marker.from_analysis is covered by findings_become_clip_markers_in_source_time, a_label_narrows_the_findings_and_re_running_adds_nothing and the locked-track and missing-analysis refusals.

Not done here, deliberately: instantiating a real WASM analyzer component. sub-plugin still depends on wasmtime without cranelift and there is no host store, fuel or epoch wiring -- that is TASK-84's scope -- so the Analyzer trait is the seam and the tests drive it with in-process analyzers. The generated analyzer-world bindings are proven to link, which is the same bar TASK-75 set for the command world.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the analyzer world to subordinate:plugin@0.1.0 and wired it into the background job queue. The WIT gains an analysis interface (markers, ranges and JSON metadata, all in media time), an analysis-host import for progress and cooperative cancellation, and world analyzer exporting analyze(media-id, options); sub-plugin generates its host bindings beside the command world, converts findings into a new sub_model::Analysis (minting the marker ids the plugin must not invent), and AnalysisJobs runs each analysis as one cancellable JobService job that stores what it found on the media item by applying media.set_analysis through the engine -- so findings arrive as an ordinary undoable command and a cancelled run stores nothing. Five new commands (media.set_analysis/remove_analysis/replace_analyses, marker.from_analysis, marker.replace_on_clip) turn findings into clip markers in source time with no rate conversion, and reach agents automatically through the regenerated docs/schema/command-api.json. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test --workspace, all clean.
<!-- SECTION:FINAL_SUMMARY:END -->
