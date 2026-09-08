---
id: TASK-2.2
title: Timecode formatting and parsing including drop-frame
status: Done
assignee:
  - '@opus-task-2.2'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-08 23:50'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-2.1
references:
  - docs/PLAN.md
parent_task_id: TASK-2
priority: high
ordinal: 17000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
NTSC-rate sources are common and drop-frame timecode is a classic source of off-by-one frame errors.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Format and parse HH:MM:SS:FF and HH;MM;SS;FF for 23.976, 24, 25, 29.97 DF and NDF, 30, 50, 59.94 DF and NDF, 60
- [x] #2 Round-trip property tests over the full 24-hour range pass for every listed rate
- [x] #3 Known drop-frame vectors (e.g. frame 17982 at 29.97 DF is 00:10:00;00) are asserted explicitly
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub-core dependency to sub-time so timecode failures are SubError with stable time.* codes (docs/DEVELOPMENT.md convention).
2. New module crates/sub-time/src/timecode.rs: TimecodeRate { Rational + drop_frame } validating supported rates (integral, or NTSC n000/1001) and drop-frame legality (nominal fps multiple of 30 at an NTSC rate only).
3. Timecode { rate, hours, minutes, seconds, frames } with integer-only conversions: from_frame_number (wrapping modulo 24h), to_frame_number, to/from RationalTime, Display formatting HH:MM:SS:FF and HH;MM;SS;FF, and FromStr-style parse against a rate.
4. Drop-frame arithmetic: d = nominal/15 frames dropped at each minute except every tenth; reject timecodes naming a dropped frame.
5. Tests: explicit drop-frame vectors (17982 -> 00:10:00;00, minute boundaries, 59.94), rejection cases, and full 24-hour round-trip over every listed rate (frame -> timecode -> string -> parse -> frame).
6. Verify cargo fmt --all --check, clippy --workspace --all-targets -D warnings, cargo test -p sub-time.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-time/src/timecode.rs (pub module sub_time::timecode, re-exporting Timecode and TimecodeRate) and a sub-core dependency so failures are SubError values with stable time.* codes.

Design decisions:
- TimecodeRate { rate: Rational, nominal_fps, drop_frame } validates the rate at construction: integral rates and NTSC n000/1001 rates only (nominal fps = numerator or numerator/1000, capped at 1000), so 24000/1001 -> 24, 30000/1001 -> 30, 60000/1001 -> 60. Drop-frame is accepted only where it is defined: a non-integral rate whose nominal fps is a multiple of 30 (29.97, 59.94); 25, 30, 23.976 and 60 are rejected with time.timecode_drop_frame_unsupported.
- Timecode { rate, hours, minutes, seconds, frames } is a frame label, not a duration: fields are always in range for the rate, hours are 0..=23 and from_frame_number wraps modulo frames_per_24h in both directions (frame -1 is 23;59;59;29 at 29.97 DF), the way a hardware counter wraps.
- Drop-frame arithmetic is integer only: d = nominal/15 labels (2 at 29.97, 4 at 59.94) are skipped at the start of every minute except each tenth. to_frame_number subtracts d * (total_minutes - total_minutes/10); from_frame_number re-inserts them with the ten-minute-block formula and rejects nothing, while Timecode::new/parse reject a label the counting skips (time.timecode_dropped_frame).
- Display writes HH:MM:SS:FF, or HH;MM;SS;FF at a drop-frame rate. parse accepts one- or two-digit fields, accepts either separator at a drop-frame rate, and rejects ';' at a non-drop-frame rate (time.timecode_syntax) since it asserts a counting rule that rate does not use.
- RationalTime bridges: to_rational_time is exact (frame count at the rate); from_rational_time rescales with Rounding::Nearest and reports time.timecode_out_of_range only when the rescale overflows i64. No floats anywhere.
- Error codes introduced (domain time, never to be renamed): time.timecode_syntax, time.timecode_out_of_range, time.timecode_dropped_frame, time.timecode_rate_unsupported, time.timecode_drop_frame_unsupported.

Validation: cargo test -p sub-time = 43 unit tests + 4 doctests passing. frame_number_round_trips_over_a_full_day_at_every_rate walks every frame of a full 24-hour day at all ten required configurations (23.976, 24, 25, 29.97 NDF, 29.97 DF, 30, 50, 59.94 NDF, 59.94 DF, 60 - about 34 million labels) asserting frame -> Timecode -> formatted string -> parse -> frame identity plus the day wrap. Explicit drop-frame vectors asserted: 17982 -> 00;10;00;00, 1799 -> 00;00;59;29, 1800 -> 00;01;00;02, 17981 -> 00;09;59;29, 107892 -> 01;00;00;00, frames_per_24h = 2589408 at 29.97 DF; at 59.94 DF 3600 -> 00;01;00;04, 35964 -> 00;10;00;00, 215784 -> 01;00;00;00, frames_per_24h = 5178816. cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (with the scratchpad GStreamer pkg-config env so sub-media builds).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added SMPTE timecode formatting and parsing to sub-time: TimecodeRate (exact Rational plus a validated drop-frame flag, nominal fps derived from integral and NTSC n000/1001 rates) and Timecode (HH:MM:SS:FF, HH;MM;SS;FF when drop-frame) with integer-only conversions to and from frame numbers and RationalTime, wrapping at 24 hours. Drop-frame counting skips nominal/15 labels every minute but each tenth, and timecodes naming a skipped label are rejected as SubError values with new stable time.* codes. Verified with cargo test -p sub-time (43 unit tests + 4 doctests), including a full 24-hour round trip of frame -> timecode -> string -> parse -> frame at all ten required rates and explicit drop-frame vectors (17982 = 00;10;00;00 at 29.97 DF, 35964 = 00;10;00;00 at 59.94 DF), with cargo fmt --all --check and cargo clippy --workspace --all-targets -- -D warnings both clean.
<!-- SECTION:FINAL_SUMMARY:END -->
