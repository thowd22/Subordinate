---
id: TASK-101
title: 'First-party plugin: OpenTimelineIO JSON importer and exporter'
status: To Do
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 04:20'
labels:
  - plugins
  - first-party
milestone: m-6
dependencies:
  - TASK-78
  - TASK-91
references:
  - docs/PLAN.md
priority: medium
ordinal: 122000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Interchange lives in plugins; OTIO is the sensible target (§3).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Exports a sequence as OTIO JSON with tracks, clips, gaps, transitions and markers
- [x] #2 Effects are documented as not exported
- [ ] #3 Imports OTIO JSON produced by the exporter (round-trip test) and OTIO files written by the reference OpenTimelineIO Python library (pip opentimelineio, v0.18.x): at least one of its shipped sample timelines and one converted from CMX 3600 EDL via its adapter, committed as fixtures with their generating script
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add plugins/otio as its own cargo workspace (like plugins/gain): otio-core (pure Rust, host-testable), otio-importer (importer world) and otio-exporter (command world), each with wit-bindgen against the shared wit directory.
2. otio-core: OTIO JSON schema types (Timeline.1, Stack.1, Track.1, Clip.1 and Clip.2, Gap.1, Transition.1, Marker.2, ExternalReference.1, RationalTime.1, TimeRange.1), exact conversion between Rational and the OTIO double rate so no timeline math is done in floats, a minimal serde mirror of the .sub project JSON, sequence-to-OTIO export and OTIO-to-import-plan parsing.
3. otio-importer: importer world, maps the import plan onto importer-api media and sequence specs. otio-exporter: command world, queries project.get, exports the named sequence, returns the JSON and optionally writes it.
4. Fixtures and tests: exporter output round-trips through the importer, plus a Kdenlive-shaped otio file (fields taken from the KDE kdenlive otioexport source) covering Clip.2 media_references, track source_range, kdenlive metadata and one-frame markers.
5. plugin.toml manifests for both components, README documenting that effects and per-clip parameters are not exported, and CI build steps mirroring plugins/gain.
6. Verify: cargo fmt check, clippy with -D warnings and tests in both the host workspace and the plugin workspace.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Shipped plugins/otio, a second self-contained cargo workspace holding three crates: otio-core (the OTIO JSON schema, both conversions and the exact-time boundary, no WIT), otio-importer (the importer world) and otio-exporter (the command world). Everything but the component glue is host-testable, so the conversion is covered by ordinary unit and integration tests and the wasm build only has to prove the glue.

Design decisions worth recording:
- The exporter implements the command world, not the exporter world. The exporter world contributes encoder presets and a post-encode hook; writing an OTIO document encodes nothing, and a command plugin already has project.get and can write inside its granted folder. Its plugin.toml and lib.rs both say so.
- OTIO serialises RationalTime as two JSON numbers, so otio-core::time is the only place a float exists. It recognises the NTSC family (n/1001) before anything else, then whole rates, then a continued-fraction search bounded at a denominator of 100000, so 24000/1001 survives a round trip through the double instead of coming back as a 15-digit approximation. Rescaling and every duration sum stay integer, and a non-integral tick count is refused rather than rounded.
- What OTIO cannot hold is written into a subordinate metadata namespace (canvas resolution, audio sample rate, ids), which is what makes a round trip through this pair lossless for the settings OTIO has no field for.
- Importing someone else's file keeps the cut and reports the loss: a clip with no ExternalReference (Kdenlive colour generators, MissingReference) becomes a gap of the same length, a Subtitle track is skipped, track markers are dropped, and each produces a note the component logs. An absolute target_url is reduced to its file name because a project stores project-relative paths.
- The three crates share one workspace, so target/ is at the workspace root and 'plugin install <wasm>' cannot find the right plugin.toml by walking up. The README documents the directory form instead (copy the built component next to its manifest, install the directory), which is how it was verified below.

Verification:
- cargo test --workspace in plugins/otio: 31 tests pass (13 core unit, 3 kdenlive, 4 roundtrip, 5 exporter, 4 importer mapping, 2 doc).
- cargo clippy --workspace --all-targets -- -D warnings and cargo fmt --all --check clean in both workspaces.
- cargo build --release --target wasm32-wasip2 --workspace produces both components (otio_importer.wasm 287 KB, otio_exporter.wasm 265 KB).
- End to end under wasmtime: installed both components with subordinate-cli plugin install into a scratch plugin dir (both loaded, status ok), then subordinate-cli plugin test com.subordinate.otio-export --fixture crates/sub-model/tests/fixtures/sample-project.sub reported 5 passed, 0 failed, 1 skipped, with the run check returning an 8.8 KB OTIO document for the sample project's 23.976 fps Main sequence: three tracks, a crossfade, a gap, a clip marker and a sequence marker.
- cargo test -p sub-plugin (211 tests) passes, including the new tests/first_party_manifests.rs, which parses both shipped plugin.toml files with the host's own Manifest::parse and asserts their worlds and capabilities.
- CI gained a Linux-only 'First-party plugins (OpenTimelineIO)' step mirroring the plugins/gain one: wasm build, tests, clippy and fmt in plugins/otio.

Acceptance criterion 2 is left unchecked, deliberately and only for its second half. The round-trip half is proven: tests/roundtrip.rs exports fixtures/project.json, compares it against a committed golden document and imports it back, asserting the media list, both track kinds, the clip source ranges, the gap, the crossfade offsets and both markers come back unchanged at 24000/1001. The Kdenlive half is exercised by tests/kdenlive.rs against fixtures/kdenlive.otio, which was written field by field against Kdenlive's own exporter (src/otio/otioexport.cpp in KDE/kdenlive) and carries its idioms: kdenlive metadata, a null global_start_time, per-track source ranges, Clip.2 media_references with an active key, absolute percent-escaped target_urls, a GeneratorReference colour clip, guides as one-frame stack markers with their text in comment, a mix as an asymmetric SMPTE_Dissolve, and a Subtitle track. It is a faithful reconstruction, not a file captured from a Kdenlive run: this machine has no sudo and no Qt or MLT, so Kdenlive cannot be installed to produce one. Running a real Kdenlive export through the importer is the one thing left to confirm the criterion.

2026-09-10 supervisor: replaced the Kdenlive half of criterion 2. A Kdenlive-authored OTIO file requires a GUI export no agent can perform; the reference OpenTimelineIO Python library is the canonical producer and can be driven headlessly (pip install opentimelineio, otioconvert). Round-trip half already proven. Requeued.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the first-party OpenTimelineIO plugins as plugins/otio: otio-core holds the OTIO schema, the sequence-to-timeline export, the timeline-to-import-plan read and the one place an exact rate crosses OTIO's f64 encoding; otio-importer implements the importer world and otio-exporter the command world (not the exporter world, which is for encoder presets). Tracks, clips, gaps, crossfades and markers cross in both directions, canvas and audio rate ride in a subordinate metadata namespace, and effects and per-clip parameters are deliberately not exported, documented in the README, the module docs and a test. Verified with 31 tests in the plugin workspace, clippy and fmt clean in both workspaces, a wasm32-wasip2 build of both components, a new sub-plugin test that parses both shipped plugin.toml files, and an end-to-end subordinate-cli plugin install plus plugin test run that exported the sample project under wasmtime (5 passed, 0 failed, 1 skipped). Acceptance criterion 2 stays unchecked for its Kdenlive half only: the fixture reproduces Kdenlive's exporter faithfully but was not captured from a Kdenlive run, which this machine cannot install.
<!-- SECTION:FINAL_SUMMARY:END -->
