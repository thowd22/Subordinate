---
id: TASK-88
title: 'Effects in the inspector: add, reorder, remove, edit parameters'
status: Done
assignee:
  - '@opus-task-88'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 07:22'
labels:
  - ui
  - plugins
milestone: m-6
dependencies:
  - TASK-87
  - TASK-37
references:
  - docs/PLAN.md
priority: medium
ordinal: 109000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users need to apply plugin effects without an agent.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Inspector lists applied effects with parameter widgets generated from ParamDesc
- [x] #2 Add-effect picker lists installed effect plugins
- [x] #3 All edits are undoable commands
- [x] #4 An egui_kittest test in `sub-ui` built on the shared harness in `crates/sub-ui/tests/support/mod.rs` covers the effect list in the inspector: a committed snapshot with effects present, and interaction tests for add, reorder and remove asserting the commands issued
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-edit: new commands/effect.rs with AddClipEffect, InsertClipEffect, RemoveClipEffect, MoveClipEffect and SetClipEffectParam, each atomic, each exact through its inverse (add -> remove -> insert carrying the whole ClipEffect); new edit.effect_not_found and edit.duplicate_effect codes; register them in register_builtin and regenerate docs/schema/command-api.json.
2. sub-ui: new effects module holding an EffectCatalog of installed effect plugins (id, name, lifted ParamDesc list) built from installed plugin manifests, with declarations filled in once the host has loaded them.
3. sub-ui inspector: an Effects section under the parameter sliders listing the anchor clip's effect stack in order, one widget per declared parameter generated from its ParamKind (slider, drag, checkbox, colour, combo), Move up / Move down / Remove buttons per effect and an Add effect picker over the catalog. The panel still mutates nothing: the response grows an effects: Vec<EffectEdit> the caller applies, parameter drags reusing the existing begin/commit gesture grouping.
4. app.rs: hold an EffectCatalog on SubordinateApp, pass it to the inspector, apply the effect edits through the session inside the same group.
5. Tests: unit tests in sub-edit for each command including undo/redo exactness, unit tests in the inspector module for widget/value mapping, and egui_kittest tests in crates/sub-ui/tests/inspector.rs on the shared harness: a committed snapshot of the inspector with effects present plus interaction tests for add, reorder and remove asserting the commands issued.
6. Verify: cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings, cargo test for sub-edit, sub-command and sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in three layers.

sub-edit: new crates/sub-edit/src/commands/effect.rs with five commands — AddClipEffect (clip.add_effect), InsertClipEffect (clip.insert_effect), RemoveClipEffect (clip.remove_effect), MoveClipEffect (clip.move_effect) and SetClipEffectParam (clip.set_effect_param). They follow the crate's two conventions: a removal's inverse carries the whole ClipEffect and its index, so undo/redo restores identity, order and every bound value; lookups fail with stable codes before anything is written (new edit.effect_not_found and edit.duplicate_effect beside the existing edit.invalid_index, edit.track_locked and model.invalid_effect). SetClipEffectParam with no value clears a parameter back to the plugin's declared default, and its inverse carries whatever was bound before. All five are registered in register_builtin, so they are on the Command API, the MCP bridge and the plugin host; docs/schema/command-api.json regenerated (SUB_UPDATE_SCHEMA=1) and docs/mcp-guide.md gained the effect tool family, which its completeness test enforces.

sub-ui: new crates/sub-ui/src/effects.rs holding EffectCatalog/EffectListing — the editor's view of installed effect plugins, built from the install scan (EffectCatalog::from_installed keeps enabled plugins whose manifest declares the effect world) and filled in with the lifted ParamDesc list once the host has loaded a plugin (EffectCatalog::declare). It holds no component and no device, so panels and tests can build one by hand.

Inspector: an Effects section under the sliders showing the anchor clip's stack in run order, one generated control per declared parameter (float/int slider, bool checkbox, choice combo, colour as four exact channel sliders), Move up / Move down / Remove per row and a picker listing every installed effect plugin. The panel still mutates nothing: it raises InspectorResponse::effects (a new EffectEdit enum wrapping the four commands) which apply_edit and SubordinateApp::apply_inspector apply. A click is one undo entry; a parameter drag reuses the existing begin/commit gesture grouping so the whole drag is one entry and the edit is live under the pointer. Effect edits act on the anchor clip only (an effect is an instance on one clip), and only when its track is unlocked.

Scope note: SubordinateApp holds an empty EffectCatalog because this window does not host a plugin runtime yet (the plugins panel is likewise not wired into the app); the picker lists what the host tells it about.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test --workspace green (exit 0), including the 16 tests of crates/sub-ui/tests/inspector.rs, the 10 new command tests in sub-edit, sub-command's committed_schema_is_up_to_date and subordinate-mcp's guide completeness test.

AC evidence:
#1 the_effect_list_shows_every_applied_effect_with_its_declared_controls drives the panel over a clip carrying two effects and asserts the row labels and one control per declared parameter; the module tests a_parameter_takes_the_plugins_default_until_the_effect_binds_one, a_bound_value_of_the_wrong_shape_falls_back_to_the_declaration and a_declared_colour_crosses_into_exact_channels pin the value mapping.
#2 a_clip_with_no_effects_says_so_and_still_offers_the_picker plus the assertions in #1 show the picker listing each catalogue entry; effects.rs's only_enabled_effect_plugins_are_offered proves the catalogue is the enabled effect-world plugins of the install scan.
#3 adding_an_effect_from_the_picker_is_one_undoable_command, moving_an_effect_down_reorders_the_stack_and_undoes, removing_an_effect_takes_it_off_and_undoes_with_its_values and dragging_an_effect_parameter_edits_live_and_commits_one_undo_step each assert exactly one undo entry and an undo that restores the previous state (identity and bound values included); sub-edit's a_redone_add_restores_the_same_identity_and_values covers redo.
#4 tests/inspector.rs on the shared harness: committed snapshot inspector_clip_effects.png with two effects present (inspector_clip_parameters.png re-recorded, since the panel now ends with the effects section), and the_effect_commands_the_panel_issues_name_the_clip_and_the_effect asserts the exact Move, Remove and Add commands the three interactions raise.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the effect-stack command set to sub-edit (clip.add_effect, clip.insert_effect, clip.remove_effect, clip.move_effect, clip.set_effect_param, with edit.effect_not_found and edit.duplicate_effect) and built the inspector's Effects section on it: the anchor clip's stack in run order, controls generated from each plugin's declared ParamDesc, move/remove per row and a picker over the installed effect plugins held in the new sub-ui EffectCatalog. The panel raises commands rather than mutating, so every edit is one undoable command through the Command API, a click one undo entry and a parameter drag one entry for the whole gesture. Verified with cargo fmt --check, clippy -D warnings, a green cargo test --workspace, ten new sub-edit command tests and seven egui_kittest tests on the shared harness including the committed inspector_clip_effects snapshot; docs/schema/command-api.json and docs/mcp-guide.md regenerated for the new methods.
<!-- SECTION:FINAL_SUMMARY:END -->
