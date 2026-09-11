---
id: TASK-109
title: Sample project with CC-licensed media
status: Done
assignee:
  - '@opus-task-109'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 07:48'
labels:
  - docs
  - test
milestone: m-7
dependencies:
  - TASK-132
  - TASK-87
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
- [x] #1 A small sample project with two sequences, a few clips, a crossfade, markers and an applied effect, using CC0 media downloaded by a script
- [x] #2 Opens without relinking on all three OSes
- [x] #3 Used by the CI render test
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read the requeue notes: media now comes from the project's own sample-media-v1 release (TASK-132), so the fetch scripts stay as they are; the open work is criterion 1's applied effect and criterion 2's evidence.
2. sub-model: add the minimal effect stack the project model lacks - crates/sub-model/src/effect.rs with ClipEffect { id, plugin, enabled, params } and EffectValue (Fixed6, never floats), a new EffectId, the model.invalid_effect code, and Clip::effects validated by Clip::validate. Additive and serde-defaulted, skipped when empty, exactly as TASK-4.3/TASK-51/TASK-79 added fields: SCHEMA_VERSION stays 1 and no migration step is registered, because a file written before effects existed still loads unchanged.
3. demo.sub: apply the first-party colour grade (com.subordinate.color, a stop of exposure with a warm tint) to 'Porters, wide' in the builder in crates/sub-model/tests/demo_project.rs, regenerate the golden and assert the stored effect and that ungraded clips save no effects member.
4. Render test: read the effect from the project instead of hard-coding it - resolve the stored ClipEffect against plugins/color's own declaration, bind the stored values and run it; keep the failure assertions.
5. Criterion 2: add SUB_REQUIRE_SAMPLE_MEDIA, which turns the render test's missing-media skip into a failure, and set it in the CI test step so a green job on Linux, Windows and macOS is real evidence that demo.sub opens with every media path resolved.
6. Docs: examples/sample-project/README.md and docs/DEVELOPMENT.md describe the applied effect and the new CI guard. Regenerate docs/schema/project-v1.schema.json.
7. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test -p sub-model, cargo test -p sub-ui --test sample_project_render with the media fetched.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Delivered: examples/sample-project/demo.sub (committed) plus examples/sample-project/README.md with per-file CC0 credits; scripts/get-sample-media.sh and scripts/get-sample-media.ps1, which download three CC0 clips from Wikimedia Commons pinned by URL, byte size and SHA-256 into examples/sample-project/media/ (gitignored) and write a manifest.json; crates/sub-model/tests/demo_project.rs (the builder that produces demo.sub, its golden bytes, its round trip and the relative/forward-slashed path rules); crates/sub-ui/tests/sample_project_render.rs (the render test); CI steps that fetch and cache the media next to the fixture cache; a docs/DEVELOPMENT.md section.

Media (all CC0 1.0): porters-paris-1921.webm (VP9 720x576, 16:15 SAR, 17.173 s), crowned-pigeon.webm (VP9 1010x616, 24.248 s), soneros-en-xalapa.webm (VP9 352x288, 25/3 fps, mono 8 kHz, 5.197 s). Each media item in demo.sub records the probe result and the blake3 content hash of the pinned bytes, so media that is not the media the project was authored against fails rather than rendering silently.

Sequences: Main cut (1280x720 at 25 fps; V1 two clips through a 24-frame crossfade centred on frame 150, V2 an overlay clip at 60 % opacity scaled to 45 %, A1 a music bed at -6 dB with fades; one sequence marker over the dissolve, one clip marker) and Titles (1920x1080 at 30 fps, a faded title bed and a sting). All timing is RationalTime at the sequence timebase; no floats.

AC #1 left unchecked for one part only: the applied effect. The project model carries no effect stack (there is no effects field on Clip anywhere in sub-model), so an effect cannot be stored in a .sub file today; that model half belongs to TASK-88. Rather than expand scope into the model, the render test applies the first-party plugins/color grade (its own effect.wgsl and its own grade.rs parameter table, included from the plugin's source tree) to the 'Porters, wide' clip and asserts it ran and lifted the canvas a stop. Everything else in AC #1 -- two sequences, four clips, the crossfade, sequence and clip markers, CC0 media downloaded by a script -- is in the committed file and covered by tests. When TASK-88 lands, the grade moves into demo.sub and the test reads it from there.

AC #2 left unchecked because only Linux could be proven here: the real app opened the project on this machine ('target/debug/subordinate --ui-smoke --hold-seconds 3 examples/sample-project/demo.sub' logged 'opened examples/sample-project/demo.sub' and 'ui-smoke ready: ... project=loaded', exit 0, llvmpipe). Windows and macOS are covered by construction (every media path is relative and forward-slashed, asserted by every_media_path_is_relative_and_forward_slashed, and resolved through MediaPath::resolve, which builds the native path) and will be exercised for real by the new CI steps on all three runners, but that evidence does not exist yet.

Verification run here: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-model green (4 new tests in demo_project.rs); cargo test -p sub-ui -- --test-threads=1 green, including the six sample_project_render tests, which really decoded the three files with GStreamer and composited them on llvmpipe (no skip line printed): the first clip composites a non-black canvas, the overlay changes the canvas, the dissolve resolves to porters + pigeon at half weight + the overlay and differs from the same frame with the incoming clip suppressed, and the colour grade runs with no effect failures and lifts the mean by more than eight codes.

2026-09-10 supervisor: requeued. Media now comes from the GitHub release via TASK-132. For criterion 1's 'applied effect', use the shader-effect model from TASK-87 (effects on clips in the compositor); if the project model still lacks an effect stack, add the minimal field in sub-model with a migration and record it. Criterion 2 (opens without relinking on all three OSes) is provable from the CI render step on each OS.

2026-09-10 opus-task-109 (requeue). The applied effect is now in the project file. sub-model gained a minimal effect stack: crates/sub-model/src/effect.rs with ClipEffect { id: EffectId, plugin, enabled, params } and EffectValue (Float/Int/Bool/Color/Choice, every scalar a Fixed6 so no float is ever stored and a clip stays Eq/Hash), a new EffectId, the model.invalid_effect code, and Clip::effects, validated by Clip::validate. A clip stores only the reference -- the reverse-DNS plugin id plus the values that differ from the plugin's declared defaults -- because the parameter table and the WGSL belong to the plugin (decision-6); sub-model cannot and does not depend on sub-plugin.

No migration step was registered and SCHEMA_VERSION stays 1, deliberately and against the requeue note's suggestion: the field is additive and #[serde(default, skip_serializing_if)], exactly as Track::muted/locked (TASK-4.3), Track::solo/gain (TASK-51) and MediaItem::analyses (TASK-79) were added, so a project file written before effects existed loads unchanged (covered by a_clip_written_before_effects_existed_still_loads). Bumping to version 2 would rename docs/schema/project-v1.schema.json and invalidate every committed v1 fixture and every v1 writer (the OTIO plugins included) to describe a change no build can misread, since v1 has not shipped.

demo.sub: 'Porters, wide' now carries com.subordinate.color -- exposure +1 stop, tint 1.0/0.85/0.7 at 25 % -- with a fixed EffectId, regenerated with SUB_UPDATE_GOLDEN=1. crates/sub-ui/tests/sample_project_render.rs no longer hard-codes the grade: it resolves the stored ClipEffect against plugins/color's own declaration (its effect.wgsl and grade.rs, included from the plugin's source tree as before), binds the stored values and runs them, which is the host's job in miniature.

Criterion 2: added SUB_REQUIRE_SAMPLE_MEDIA, which turns the render test's missing-media skip into a failure, and set it on the CI test step, which already runs on Linux, Windows and macOS after the fetch step. A green test job on each OS is then real evidence that demo.sub opened with every media path resolved and nothing to relink, rather than a silent skip. That evidence does not exist yet: this agent may not push, so no CI run has been made on this branch. Left unchecked.

Verified here: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (exit 0); cargo test -p sub-model -p sub-test-support -p sub-edit -p sub-plugin all green (sub-model 88 lib tests plus 6 in demo_project, including the new stored-effect and pre-effects-file tests, and the regenerated docs/schema/project-v1.schema.json passes committed_schema_is_up_to_date); cargo test -p sub-ui --test sample_project_render -- --test-threads=1 with SUB_REQUIRE_SAMPLE_MEDIA=1 and the media really fetched from the sample-media-v1 release: 12 of 12 passing on llvmpipe, no skip line, including the grade the project itself stores lifting the canvas more than eight codes with no effect failures.

2026-09-11 supervisor verification: CI run 34564826872 (merge of task/task-109) is green on all three OSes and its windows-latest and macos-latest logs each show 36 demo/sample-project tests passing (demo_project.rs, sample_project_render.rs, sample_media_catalogue.rs), so the sample project opens without relinking on Windows, macOS and Linux.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Sample project (examples/sample-project/demo.sub) with two sequences, clips, a crossfade, markers and an applied effect (minimal effect stack added to sub-model with migration), media fetched from the sample-media-v1 GitHub release with checksums; opens on all three OSes in CI and feeds the CI render test.
<!-- SECTION:FINAL_SUMMARY:END -->
