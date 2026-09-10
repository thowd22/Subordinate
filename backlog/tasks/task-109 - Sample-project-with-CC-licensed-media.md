---
id: TASK-109
title: Sample project with CC-licensed media
status: In Progress
assignee:
  - '@opus-task-109'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 22:26'
labels:
  - docs
  - test
milestone: m-7
dependencies:
  - TASK-100
references:
  - docs/PLAN.md
priority: medium
ordinal: 130000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A ready-made project demonstrates the editor and feeds the CI render test.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A small sample project with two sequences, a few clips, a crossfade, markers and an applied effect, using CC0 media downloaded by a script
- [ ] #2 Opens without relinking on all three OSes
- [x] #3 Used by the CI render test
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Pick three small CC0 clips on Wikimedia Commons (verified CC0, pinned by URL and SHA-256) and probe them with sub-media for real duration, resolution and frame rate.
2. Add scripts/get-sample-media.sh and scripts/get-sample-media.ps1: download the pinned files into examples/sample-project/media/, verify SHA-256, skip what is present, --list/--force/--dry-run, write media/manifest.json. Media itself stays gitignored.
3. Commit examples/sample-project/demo.sub: two sequences, a few clips over three tracks, a crossfade, sequence and clip markers, media items with relative forward-slash paths so the project opens without relinking on any OS.
4. Add examples/sample-project/README.md with per-file credits, licence deeds and source URLs, and reference it from docs/DEVELOPMENT.md.
5. Add crates/sub-ui/tests/sample_project_render.rs, the CI render test: load demo.sub, decode NV12 frames of the real media with sub-media, composite them through sub-render at chosen times (including the crossfade midpoint), apply the first-party plugins/color grade to one clip, and assert the composite is a genuine blend and the grade ran. Skip cleanly when the media has not been fetched or the machine has no wgpu adapter.
6. Add a round-trip test that demo.sub loads and saves byte-identically and that its media paths are relative and forward-slashed.
7. Wire the fetch step into .github/workflows/ci.yml with its own cache entry, next to the fixture cache.
8. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test -p sub-ui -p sub-model.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Delivered: examples/sample-project/demo.sub (committed) plus examples/sample-project/README.md with per-file CC0 credits; scripts/get-sample-media.sh and scripts/get-sample-media.ps1, which download three CC0 clips from Wikimedia Commons pinned by URL, byte size and SHA-256 into examples/sample-project/media/ (gitignored) and write a manifest.json; crates/sub-model/tests/demo_project.rs (the builder that produces demo.sub, its golden bytes, its round trip and the relative/forward-slashed path rules); crates/sub-ui/tests/sample_project_render.rs (the render test); CI steps that fetch and cache the media next to the fixture cache; a docs/DEVELOPMENT.md section.

Media (all CC0 1.0): porters-paris-1921.webm (VP9 720x576, 16:15 SAR, 17.173 s), crowned-pigeon.webm (VP9 1010x616, 24.248 s), soneros-en-xalapa.webm (VP9 352x288, 25/3 fps, mono 8 kHz, 5.197 s). Each media item in demo.sub records the probe result and the blake3 content hash of the pinned bytes, so media that is not the media the project was authored against fails rather than rendering silently.

Sequences: Main cut (1280x720 at 25 fps; V1 two clips through a 24-frame crossfade centred on frame 150, V2 an overlay clip at 60 % opacity scaled to 45 %, A1 a music bed at -6 dB with fades; one sequence marker over the dissolve, one clip marker) and Titles (1920x1080 at 30 fps, a faded title bed and a sting). All timing is RationalTime at the sequence timebase; no floats.

AC #1 left unchecked for one part only: the applied effect. The project model carries no effect stack (there is no effects field on Clip anywhere in sub-model), so an effect cannot be stored in a .sub file today; that model half belongs to TASK-88. Rather than expand scope into the model, the render test applies the first-party plugins/color grade (its own effect.wgsl and its own grade.rs parameter table, included from the plugin's source tree) to the 'Porters, wide' clip and asserts it ran and lifted the canvas a stop. Everything else in AC #1 -- two sequences, four clips, the crossfade, sequence and clip markers, CC0 media downloaded by a script -- is in the committed file and covered by tests. When TASK-88 lands, the grade moves into demo.sub and the test reads it from there.

AC #2 left unchecked because only Linux could be proven here: the real app opened the project on this machine ('target/debug/subordinate --ui-smoke --hold-seconds 3 examples/sample-project/demo.sub' logged 'opened examples/sample-project/demo.sub' and 'ui-smoke ready: ... project=loaded', exit 0, llvmpipe). Windows and macOS are covered by construction (every media path is relative and forward-slashed, asserted by every_media_path_is_relative_and_forward_slashed, and resolved through MediaPath::resolve, which builds the native path) and will be exercised for real by the new CI steps on all three runners, but that evidence does not exist yet.

Verification run here: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-model green (4 new tests in demo_project.rs); cargo test -p sub-ui -- --test-threads=1 green, including the six sample_project_render tests, which really decoded the three files with GStreamer and composited them on llvmpipe (no skip line printed): the first clip composites a non-black canvas, the overlay changes the canvas, the dissolve resolves to porters + pigeon at half weight + the overlay and differs from the same frame with the incoming clip suppressed, and the colour grade runs with no effect failures and lifts the mean by more than eight codes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the sample project at examples/sample-project: demo.sub with two sequences, four clips, a crossfade and markers, three CC0 clips fetched by scripts/get-sample-media.sh, a model-side golden and path test, and a render test in sub-ui that decodes and composites the real media.
<!-- SECTION:FINAL_SUMMARY:END -->
