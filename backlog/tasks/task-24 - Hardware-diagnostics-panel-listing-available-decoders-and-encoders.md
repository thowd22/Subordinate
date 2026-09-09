---
id: TASK-24
title: Hardware diagnostics panel listing available decoders and encoders
status: Done
assignee:
  - '@opus-task-24'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 04:35'
labels:
  - ui
  - media
milestone: m-1
dependencies:
  - TASK-14
references:
  - docs/PLAN.md
priority: medium
ordinal: 45000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Missing GStreamer elements is the most likely support issue (§9); users and agents need to see what was detected.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Panel lists detected decoders and encoders per vendor (nvcodec, va, amf, vtenc, mf, x264) with versions
- [x] #2 Same information is available via subordinate-cli diag as JSON
- [x] #3 Missing expected elements show a hint with the install step from DEVELOPMENT.md
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New sub-media module `diagnostics`: a static catalogue of expected decoder/encoder elements per vendor (nvcodec, va, amf, applemedia/vtenc, mediafoundation, software x264/x265/libav), each with kind, vendor, and the platforms it is expected on.
2. `HardwareDiagnostics::collect()` initialises GStreamer, reads the registry (ElementFactory::find, plugin version, rank) and reports per-vendor groups with plugin versions, present/missing elements, and an install hint from docs/DEVELOPMENT.md for vendors expected on this platform that are missing elements. Serde-serialisable; errors are SubError with existing media codes.
3. subordinate-cli gains a `diag` subcommand printing the report as JSON on stdout (plus --help/unknown-arg handling); depends on sub-media and serde_json.
4. sub-ui gains a diagnostics panel (crates/sub-ui/src/diagnostics.rs) that lazily collects the report and lists vendors, versions, present elements and hints; pure formatting helpers kept out of egui so they are unit-testable.
5. Tests: catalogue well-formedness, platform expectations, hint text carries the DEVELOPMENT.md step, JSON shape stability, CLI diag integration test asserting parseable JSON, UI formatting tests. Document the panel and the diag command in docs/DEVELOPMENT.md.
6. Verify: cargo fmt --all --check, clippy -D warnings, tests for sub-media, sub-ui, subordinate-cli (with the local GStreamer env).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in three layers.

sub-media::diagnostics is the source of truth: a fixed catalogue of 24 elements grouped into six vendors (nvcodec, va, amf, vtenc, mf, x264), scanned through the GStreamer registry (ElementFactory::find, plugin name, plugin version, factory rank). HardwareDiagnostics::collect() only fails with the existing media.init_failed code - a missing element is a result, not an error. Each vendor knows which platforms it is expected on, and an expected vendor missing any element carries the matching install step from docs/DEVELOPMENT.md as its hint. The report is serde-serialisable and round-trips.

subordinate-cli gained argument parsing and a diag subcommand printing that report as JSON (pretty by default, --compact for one line); unknown arguments now fail with usage instead of being ignored, and a bare invocation still prints the version so the CI smoke test is unchanged.

sub-ui gained DiagnosticsPanel (a window opened from the main view) showing the same report: a summary line, one collapsing header per vendor with its plugin version and decoder/encoder counts, one line per element with its plugin and version, and the install hint in the warning colour for incomplete expected vendors. The scan is cached; Rescan clears it.

Validation on this machine (GStreamer 1.24.2, Linux, no NVIDIA or VA plugins): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-media -p sub-ui -p subordinate-cli all green (28 + 9 + 7 sub-media, 7 sub-ui, 4 + 3 + 4 subordinate-cli). AC1 evidence: the panel is painted into a headless egui context and the test asserts every vendor label, every element name of the expected vendors and every install hint appear in the drawn text; the x264 family reports 4/4 present with plugin versions. AC2 evidence: tests/diag.rs runs the built binary and parses its stdout, asserting the vendor order, element shape and hint text. AC3 evidence: hints are asserted to cite docs/DEVELOPMENT.md for every expected vendor, in unit tests and through the CLI.

Not verifiable here: no NVIDIA, VA-API, AMF, VideoToolbox or Media Foundation elements exist on this machine, so the present-element path for hardware vendors is exercised with a stubbed registry lookup rather than real hardware.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added a hardware diagnostics report: sub-media::diagnostics scans the GStreamer registry for the 24 decoder and encoder elements the editor uses, grouped into six vendors with plugin versions and ranks, and attaches the docs/DEVELOPMENT.md install step to any vendor that is expected on the current platform but incomplete. subordinate-cli diag prints it as JSON and the new sub-ui DiagnosticsPanel shows it in the editor. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, and the sub-media, sub-ui and subordinate-cli test suites, including a headless egui pass asserting the painted panel text and an integration test parsing the CLI JSON.
<!-- SECTION:FINAL_SUMMARY:END -->
