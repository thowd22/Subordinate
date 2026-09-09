---
id: TASK-3.2
title: 'Per-clip parameters: source range, placement, opacity, transform, gain, fades'
status: Done
assignee:
  - '@opus-task-3.2'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 00:23'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-3.1
references:
  - docs/PLAN.md
parent_task_id: TASK-3
priority: high
ordinal: 20000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The MVP supports a fixed set of clip parameters (§2, §5.1); modelling them explicitly keeps the compositor and mixer simple.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Clip has source_range and timeline placement as TimeRange, plus opacity, position/scale/rotation transform, gain in dB, fade in/out durations
- [x] #2 Invariants (non-negative durations, fades not exceeding clip length) are validated by a validate() method returning SubError
- [x] #3 Unit tests cover valid and invalid parameter sets
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add crates/sub-model/src/params.rs with float-free fixed-point parameter newtypes: Fixed6 (six-decimal fixed point over i64), Opacity, GainDb, Scale/Vec2 position and Transform (position, scale, rotation). Constructors validate and return SubError; as_f32/as_f64 accessors serve the compositor and mixer.
2. Extend Clip in track.rs with opacity, transform, gain, fade_in and fade_out (RationalTime durations). Defaults are identity: fully opaque, identity transform, unity gain, no fades.
3. Timeline placement: keep OTIO positional layout as the single source of truth and expose placement as a TimeRange via Clip::timeline_range(start) and Track::placements()/timeline_range_of(clip).
4. Clip::validate() -> SubResult<()> checking non-negative source range and fade durations and fade_in + fade_out <= clip duration; new stable code model.invalid_clip (plus model.invalid_parameter for parameter constructors).
5. Unit tests for valid and invalid parameter sets, placement math, and error codes; run fmt, clippy pedantic -D warnings and cargo test -p sub-model.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-model.

New module params.rs. Non-time clip parameters are Fixed6, an exact six-decimal fixed-point scalar over i64 micro-units, not floats: clips stay Eq/Hash, saved projects stay byte-identical across platforms and a NaN is unrepresentable. Floats appear only in the from_f64/as_f32 conversions at the edges (UI sliders in, compositor and mixer out). Types: Fixed6, Opacity (0..=1), GainDb (-144..=+24 dB), Point2, Scale2 (non-zero axes, negative mirrors) and Transform (position, scale, rotation_degrees; IDENTITY default).

Clip gained opacity, transform, gain, fade_in and fade_out (RationalTime); Clip::new defaults to opaque, identity, unity and no fades.

Timeline placement is exposed as a TimeRange rather than stored: the model keeps OTIO positional layout as the single source of truth (a stored start would duplicate state that the item order already fixes), so placement comes from Clip::timeline_range(start), Track::placements(rate), Track::clip_placements(rate) and Track::timeline_range_of(clip_id, rate). Transitions place as empty ranges at the cut.

Clip::validate() -> SubResult<()> checks non-negative source-range duration, non-negative fades and fade_in + fade_out <= duration, comparing exactly across rates via RationalTime. New stable codes: model.invalid_clip and model.invalid_parameter; errors carry clip_id, clip_name and the offending values as details.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings exit 0 (with the scratchpad GStreamer env sourced so sub-media builds); cargo test -p sub-model 39 unit tests + 2 doctests pass, including 7 new tests covering valid and invalid parameter sets and 3 covering placement.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the MVP per-clip parameter set to sub-model: a new params module of exact fixed-point types (Fixed6, Opacity, GainDb, Point2, Scale2, Transform) and Clip fields for opacity, transform, gain and fade in/out, alongside the existing source_range. Timeline placement is derived from OTIO positional layout and exposed as a TimeRange (Clip::timeline_range, Track::placements/clip_placements/timeline_range_of) rather than stored twice. Clip::validate() enforces non-negative durations and fades that do not exceed the clip, returning SubError with the new stable codes model.invalid_clip and model.invalid_parameter. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings (exit 0) and cargo test -p sub-model (39 unit tests + 2 doctests pass, 10 of them new).
<!-- SECTION:FINAL_SUMMARY:END -->
