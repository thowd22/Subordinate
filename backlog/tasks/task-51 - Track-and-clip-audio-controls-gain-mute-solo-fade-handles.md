---
id: TASK-51
title: 'Track and clip audio controls: gain, mute, solo, fade handles'
status: Done
assignee:
  - '@opus-task-51'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-10 19:24'
labels:
  - ui
  - audio
milestone: m-3
dependencies:
  - TASK-48
  - TASK-31
references:
  - docs/PLAN.md
priority: medium
ordinal: 72000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Basic audio manipulation UI (§2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Audio clips show fade handles at both ends draggable on the timeline
- [x] #2 Track headers gain mute and solo; inspector shows clip gain in dB
- [x] #3 All changes commit undoable commands and apply live
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Model: add solo and gain (GainDb, default unity) to sub-model Track, serde-defaulted so old files load; regenerate the golden fixture and project schema.
2. Commands: add SetTrackSolo and SetTrackGain in sub-edit commands/track.rs beside SetTrackMuted, register them in register_builtin, regenerate docs/schema/command-api.json.
3. Track header UI: layout rects and controls for solo and a dB gain drag on audio tracks, raising new TrackAction variants that map onto the two commands; gain drags are live and group into one undo entry.
4. Timeline fade handles: new sub-ui fade module planning a fade drag on an audio clip into a SetClipParams (clamped to the clip and to the other fade), a Fade gesture in timeline_panel with hit targets at the clip's top corners, a painted ramp and handle, and the plan reported on release.
5. Tests: model defaults and round-trip, command round-trip/undo, fade planning and clamping unit tests, headless UI tests for the solo/gain controls and a fade handle drag.
6. Verify with cargo fmt --check, clippy -D warnings on the workspace, and cargo test for sub-model, sub-edit and sub-ui.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Model: Track gained `solo: bool` and `gain: GainDb` (both serde-defaulted, so older project files still load); the golden fixture and docs/schema/project-v1.schema.json were regenerated. Commands: SetTrackSolo (track.set_solo) and SetTrackGain (track.set_gain) sit beside SetTrackMuted, are registered in the built-in registry and appear in the regenerated docs/schema/command-api.json.

UI: HeaderLayout now takes the track kind and lays out a solo toggle and a dB gain field on audio headers only (a video header keeps the layout it had, and gives the room back to the meter). A gain drag raises one SetTrackGain a frame — the mixer follows the pointer — inside one undo group carried on TimelineResponse.actions_begin/actions_commit; track_header::apply_actions applies that group. New sub-ui fade module plans a fade handle drag into one SetClipParams, clamped to the clip and to the fade at the other end, with refusals (ui.clip_fade_refused) for a locked track or a clip that has left the sequence. The timeline paints both ramps and their grips on every audio clip, hit-tests the handles in a band across the top of the clip (a trim below it, as before) and previews the drag live, committing on release exactly as a trim or a move does — the panel plans, the caller applies, since engine wiring is still a later task.

Verified: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test for the workspace, and again for sub-ui, sub-edit, sub-model, sub-command, subordinate-cli and subordinate-sdk — 52 test binaries, no failures. Six egui_kittest snapshots were re-recorded on this machine's software adapter (the audio headers and audio clips changed on purpose); every unchanged snapshot still matched byte for byte, which is why re-recording here is trustworthy.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added track solo and track gain to the model, the two commands that set them and the audio-only header controls that raise them, plus draggable fade handles on audio clips backed by a new sub-ui fade planner over SetClipParams. Verified with new headless egui_kittest tests (timeline_fades: handles painted at both ends, both edges dragged and undone, clamping; track_headers: solo click and a live gain drag that is one undo entry; inspector: clip gain read and edited in dB), new sub-edit command round-trip tests, regenerated golden fixture and schemas, and a clean cargo fmt/clippy -D warnings/cargo test run.
<!-- SECTION:FINAL_SUMMARY:END -->
