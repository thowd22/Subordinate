---
id: TASK-101
title: 'First-party plugin: OpenTimelineIO JSON importer and exporter'
status: Done
assignee:
  - '@opus-task-101'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 04:28'
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
- [x] #3 Imports OTIO JSON produced by the exporter (round-trip test) and OTIO files written by the reference OpenTimelineIO Python library (pip opentimelineio, v0.18.x): at least one of its shipped sample timelines and one converted from CMX 3600 EDL via its adapter, committed as fixtures with their generating script
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Install the reference OpenTimelineIO Python library (0.18.1) plus otio-cmx3600-adapter and use it to produce two fixtures: one of the reference repo's shipped sample timelines (tests/sample_data at tag v0.18.1) re-serialised by the library's otio_json adapter, and one timeline converted from a CMX 3600 EDL through the cmx_3600 adapter.
2. Commit both fixtures under plugins/otio/otio-core/tests/fixtures/ together with the generating script (scripts/generate_reference_fixtures.py) that downloads the pinned sources and regenerates them, with its provenance and requirements documented in the script header and the README.
3. Add plugins/otio/otio-core/tests/reference.rs: import both fixtures through import_document and assert the cut survives (rate, tracks, clips, source ranges, gaps, transitions, media) and that whatever the model cannot hold is reported in notes rather than dropped silently.
4. Fix any importer defects the real reference files expose, staying inside the existing conversion design (exact rational time, notes for loss).
5. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test --workspace in plugins/otio, plus cargo test -p sub-plugin in the host workspace. Then finalise the task.
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

2026-09-10 (wave 9): completed criterion 3, the half that had been left open. The Kdenlive half was replaced by the reference OpenTimelineIO Python library, which can be driven headlessly here.

- Installed opentimelineio 0.18.1 and otio-cmx3600-adapter 1.0.0 (pip --target into a scratch directory; this machine has no python3-venv and no sudo) and used them to produce three fixtures, committed unedited under otio-core/tests/fixtures/: reference_multitrack.otio (the reference repo's own tests/sample_data/multiple_track.otio at tag v0.18.1, read and rewritten through its otio_json adapter), reference_nucoda_edl.otio and reference_screening_edl.otio (nucoda_example.edl and screening_example.edl from the cmx3600 adapter's samples at v1.0.0, converted with the cmx_3600 adapter at 24 fps).
- plugins/otio/scripts/generate-reference-fixtures.py is the generating script: it pins both library versions and both upstream tags, downloads the sources rather than vendoring them, refuses to run against a different opentimelineio version, and has a --check mode that regenerates in memory and diffs against what is committed. --check passes against the committed files, so the fixtures are reproducible. It needs the network, so it is run by hand and not from CI; CI reads the committed fixtures.
- otio-core/tests/reference.rs imports all three and asserts what survived: 24 fps exactly from global_start_time in one file and from a track span in the other two, the three video tracks and their names, file:// URLs and Windows FROM FILE paths reduced to project-relative file names with one media item for the file used on two tracks, every source range and gap length frame-exact (86501 frames for 01:00:04:05 at 24 fps), and the screening EDL's nine MissingReference events importing as nine gaps of the same lengths (1049 frames total) rather than as invented media.
- One importer change: a clip that imports as a gap because it names no file now also notes the markers that went with it, which the screening EDL's three * LOC markers exposed. The cut was already right; the loss was silent.
- Verification: cargo test --workspace in plugins/otio now 34 tests (13 core unit, 3 kdenlive, 3 reference, 4 roundtrip, 5 exporter, 4 importer, 2 doc), all passing; cargo clippy --workspace --all-targets -- -D warnings and cargo fmt --all --check clean. No host-workspace crate was touched.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
plugins/otio ships the first-party OpenTimelineIO pair: otio-core holds the OTIO schema, the sequence-to-timeline export, the timeline-to-import-plan read and the single place an exact rate crosses OTIO's f64 encoding; otio-importer implements the importer world and otio-exporter the command world. Tracks, clips, gaps, crossfades and markers cross in both directions, canvas and audio rate ride in a subordinate metadata namespace, and effects and per-clip parameters are deliberately not exported, documented in the README and asserted in a test. Interchange is proven against the reference implementation, not only against itself: three fixtures written by the OpenTimelineIO Python library 0.18.1 (one of its shipped sample timelines plus two CMX 3600 EDLs converted with the cmx_3600 adapter) are committed with the pinned generating script that reproduces them, and tests/reference.rs imports all three with frame-exact source ranges, media paths and gap lengths. Verified with 34 tests in the plugin workspace, clippy and fmt clean, a wasm32-wasip2 build of both components, and an earlier end-to-end subordinate-cli plugin install plus plugin test run under wasmtime.
<!-- SECTION:FINAL_SUMMARY:END -->
