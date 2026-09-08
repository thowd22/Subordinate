---
id: TASK-2.1
title: RationalTime type with exact arithmetic and rescaling
status: Done
assignee:
  - '@opus-task-2.1'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-08 22:35'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-1.1
references:
  - docs/PLAN.md
parent_task_id: TASK-2
priority: high
ordinal: 16000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
All timeline math uses integer rational time so edits never drift (§5.1).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 RationalTime { value: i64, rate: Rational } supports add, sub, neg, compare, min, max with no floating point
- [x] #2 rescaled_to(rate) is exact when representable and documents its rounding mode otherwise
- [x] #3 TimeRange with start and duration supports contains, overlaps, clamp and intersection
- [x] #4 Unit tests cover mixed-rate arithmetic including 24000/1001
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add Rational (u32 numerator/denominator, reduced, non-zero) with common frame-rate consts including 24000/1001.
2. Add RationalTime { value: i64, rate: Rational } with integer-only add/sub/neg (exact common-rate via lcm), manual PartialEq/Eq/Ord/Hash on the normalised seconds fraction using i128 cross-multiplication, min/max, checked_* variants.
3. Add rescaled_to(rate) plus rescaled_to_rounding/rescaled_to_exact; document round-half-away-from-zero as the default rounding mode and exactness rule.
4. Add TimeRange { start, duration } with non-negative duration invariant: contains, overlaps, clamp, intersection, end_exclusive.
5. Unit tests incl. mixed-rate arithmetic with 24000/1001 and 30000/1001, rescaling exactness/rounding, range ops.
6. Verify with cargo fmt --check, clippy -D warnings, cargo test -p sub-time.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/sub-time as three modules re-exported from lib.rs: rational.rs (Rational rate, u32/u32 reduced, positive, consts for 24, 25, 30, 50, 60, 23.976=24000/1001, 29.97, 59.94, 48 kHz, plus common_rate = lcm(num)/gcd(den)), rational_time.rs (RationalTime { value: i64, rate: Rational }) and time_range.rs (TimeRange { start, duration }).

Decisions:
- Arithmetic is integer only. Mixed-rate add/sub first rescales both operands to the smallest exactly-representing common rate, so 1@24000/1001 + 1@30000/1001 = 9@120000/1001 with no rounding; equal rates keep the rate.
- Equality and ordering compare wall-clock length by i128 cross-multiplication (1@24 == 2@48); Hash is implemented over the reduced seconds fraction so it agrees with Eq. min/max come from Ord.
- rescaled_to is exact whenever representable (is_exactly_representable_at / rescaled_to_exact expose that); otherwise it rounds to nearest with ties away from zero, documented on the method. rescaled_to_rounding takes Rounding::{Nearest,Floor,Ceil,Trunc}.
- Overflow never wraps: checked_add/checked_sub/checked_neg/checked_abs/checked_rescaled_to* return None; the operator impls and rescaled_to panic with a documented # Panics section.
- TimeRange is half-open [start, start+duration) with a non-negative duration enforced in the constructor, so butt-joined clips neither overlap nor gap. clamp pins into [start, end_exclusive]; intersection returns None for non-overlapping or empty ranges; also from_start_end, contains_range, shifted_by, end_exclusive, is_empty.
- No error type is introduced: SubError with stable codes is TASK-11 and does not exist yet, and every fallible operation here is a total Option-returning predicate, so nothing was stubbed ahead of that task.

Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the scratchpad GStreamer pkg-config env so sub-media builds); cargo test -p sub-time = 30 unit tests plus 1 doctest, all passing.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the sub-time primitives: Rational (exact positive rate, NTSC rates such as 24000/1001 held exactly), RationalTime { value: i64, rate: Rational } with float-free add/sub/neg/compare/min/max plus checked variants and rescaled_to (exact when representable, otherwise nearest with ties away from zero, and Rounding::{Nearest,Floor,Ceil,Trunc} on rescaled_to_rounding), and half-open TimeRange with contains, contains_range, overlaps, clamp, intersection and shifted_by. Mixed-rate arithmetic is exact because operands are first rescaled to the smallest common rate (1@24000/1001 + 1@30000/1001 = 9@120000/1001). Verified with cargo test -p sub-time (30 unit tests + 1 doctest passing, covering mixed-rate NTSC arithmetic, rounding modes on ties and negatives, overflow, and range ops), cargo fmt --all --check and cargo clippy --workspace --all-targets -- -D warnings, both clean.
<!-- SECTION:FINAL_SUMMARY:END -->
