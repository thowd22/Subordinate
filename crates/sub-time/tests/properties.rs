//! Property-based tests for `sub-time` (TASK-2.3).
//!
//! Rational arithmetic bugs hide in odd inputs — NTSC rates, negatives, ties
//! and mixed-rate operands — so the invariants of `RationalTime`, `TimeRange`
//! and `Timecode` are checked against generated values rather than hand-picked
//! ones.
//!
//! The strategies below cover every rate the crate names as supported plus
//! arbitrary reduced rationals; values are bounded so that exact arithmetic
//! stays inside `i64` and a failure means a real bug, not an overflow the API
//! already reports. Case counts are kept modest so the whole suite runs in a
//! couple of seconds in CI.

use proptest::prelude::*;
use sub_time::{Rational, RationalTime, Rounding, TimeRange, Timecode, TimecodeRate};

/// Every rate the crate names as supported.
const SUPPORTED_RATES: &[Rational] = &[
    Rational::ONE,
    Rational::FPS_23_976,
    Rational::FPS_24,
    Rational::FPS_25,
    Rational::FPS_29_97,
    Rational::FPS_30,
    Rational::FPS_50,
    Rational::FPS_59_94,
    Rational::FPS_60,
    Rational::HZ_48000,
];

/// Bound on generated unit counts. Large enough to exercise carries, small
/// enough that rescaling between any two generated rates stays inside `i64`.
const VALUE_BOUND: i64 = 100_000_000;

/// A rate drawn from the named supported set.
fn supported_rate() -> impl Strategy<Value = Rational> {
    proptest::sample::select(SUPPORTED_RATES)
}

/// A rate drawn from the supported set or built from arbitrary parts.
///
/// Arbitrary parts are bounded so that a common rate of any two generated
/// rates still fits in a `u32`; NTSC-shaped denominators are included so that
/// `n/1001` rates are hit often.
fn any_rate() -> impl Strategy<Value = Rational> {
    prop_oneof![
        6 => supported_rate(),
        3 => (1_u32..=100_000, prop::sample::select(&[1_u32, 2, 4, 1001][..]))
            .prop_map(|(n, d)| Rational::new(n, d).expect("both parts are non-zero")),
        1 => (1_u32..=1_000, 1_u32..=1_001)
            .prop_map(|(n, d)| Rational::new(n, d).expect("both parts are non-zero")),
    ]
}

/// A time at one of the supported or arbitrary rates.
fn any_time() -> impl Strategy<Value = RationalTime> {
    (any_rate(), -VALUE_BOUND..=VALUE_BOUND)
        .prop_map(|(rate, value)| RationalTime::new(value, rate))
}

/// A time at one of the named supported rates.
fn supported_time() -> impl Strategy<Value = RationalTime> {
    (supported_rate(), -VALUE_BOUND..=VALUE_BOUND)
        .prop_map(|(rate, value)| RationalTime::new(value, rate))
}

/// A non-negative duration, as `TimeRange` requires.
fn any_duration() -> impl Strategy<Value = RationalTime> {
    (any_rate(), 0..=VALUE_BOUND).prop_map(|(rate, value)| RationalTime::new(value, rate))
}

/// A range built from a generated start and duration, skipping the pairs whose
/// end is not exactly representable.
fn any_range() -> impl Strategy<Value = TimeRange> {
    (any_time(), any_duration()).prop_filter_map("range end must be representable", |(s, d)| {
        TimeRange::new(s, d)
    })
}

/// A range whose bounds are all at named supported rates.
///
/// Any two supported rates have a common rate inside `u32`, so operations that
/// build a new range from two of them (an intersection, say) can never fail for
/// want of one; arbitrary rates can exceed that, which the API reports as
/// `None` rather than as a wrong answer.
fn supported_range() -> impl Strategy<Value = TimeRange> {
    let duration =
        (supported_rate(), 0..=VALUE_BOUND).prop_map(|(rate, v)| RationalTime::new(v, rate));
    (supported_time(), duration).prop_filter_map("range end must be representable", |(s, d)| {
        TimeRange::new(s, d)
    })
}

/// A timecode rate: every supported rate that timecode can label, drop-frame
/// where the rate defines it.
///
/// Nominal rates stay below 100 fps, the range SMPTE labels: a label at a
/// higher rate needs a three-digit frame field, which `Timecode::parse` (two
/// digits per field) does not accept.
fn any_timecode_rate() -> impl Strategy<Value = TimecodeRate> {
    let rates = prop_oneof![
        4 => supported_rate(),
        1 => (1_u32..=99).prop_map(|n| Rational::from_integer(n).expect("non-zero")),
        1 => prop::sample::select(&[24_000_u32, 30_000, 48_000, 60_000][..])
            .prop_map(|n| Rational::new(n, 1001).expect("non-zero")),
    ];
    (rates, any::<bool>()).prop_filter_map("rate must accept the flag", |(rate, drop)| {
        TimecodeRate::new(rate, drop && TimecodeRate::rate_drops_frames(rate)).ok()
    })
}

/// The exact seconds of `t` as a `(numerator, denominator)` pair, used as an
/// independent model of what the type's own ordering should say.
fn seconds_pair(t: RationalTime) -> (i128, i128) {
    (
        i128::from(t.value()) * i128::from(t.rate().denominator()),
        i128::from(t.rate().numerator()),
    )
}

/// Compares two times through the model rather than through `Ord`.
fn model_cmp(a: RationalTime, b: RationalTime) -> core::cmp::Ordering {
    let (an, ad) = seconds_pair(a);
    let (bn, bd) = seconds_pair(b);
    (an * bd).cmp(&(bn * ad))
}

fn hash_of(t: RationalTime) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    t.hash(&mut hasher);
    hasher.finish()
}

/// The strategy must be able to produce every named supported rate; a rate
/// added to the crate without being added here would silently go untested.
#[test]
fn strategy_covers_every_named_supported_rate() {
    let named = [
        Rational::ONE,
        Rational::FPS_23_976,
        Rational::FPS_24,
        Rational::FPS_25,
        Rational::FPS_29_97,
        Rational::FPS_30,
        Rational::FPS_50,
        Rational::FPS_59_94,
        Rational::FPS_60,
        Rational::HZ_48000,
    ];
    for rate in named {
        assert!(
            SUPPORTED_RATES.contains(&rate),
            "supported_rate() does not generate {rate}"
        );
    }
    assert_eq!(SUPPORTED_RATES.len(), named.len());
}

/// 1024 cases per property keeps the whole file well inside a second on a
/// laptop, and failures are persisted next to this file (the default location
/// resolves relative to a `lib.rs`, which an integration test has not got) so a
/// counterexample found in CI is replayed first on the next run.
fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 1024,
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(
                "tests/properties.proptest-regressions",
            ),
        )),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// Rescaling to a rate that represents the value exactly loses nothing:
    /// the result is equal, and converting back reproduces the original.
    #[test]
    fn exact_rescale_round_trip_is_identity(t in any_time(), rate in any_rate()) {
        if let Some(there) = t.rescaled_to_exact(rate) {
            prop_assert_eq!(there, t);
            prop_assert_eq!(there.rate(), rate);
            prop_assert_eq!(there.as_seconds_fraction(), t.as_seconds_fraction());
            let back = there.rescaled_to_exact(t.rate());
            prop_assert_eq!(back, Some(t));
            prop_assert_eq!(back.expect("just checked").value(), t.value());
        }
    }

    /// A rate that is a whole multiple of the source rate always represents it
    /// exactly, so this round trip is guaranteed rather than conditional.
    #[test]
    fn multiplied_rate_round_trip_is_identity(t in supported_time(), factor in 1_u32..=16) {
        let rate = t.rate();
        let Some(finer) = rate
            .numerator()
            .checked_mul(factor)
            .and_then(|n| Rational::new(n, rate.denominator()))
        else {
            return Ok(());
        };
        prop_assert!(t.is_exactly_representable_at(finer));
        let there = t.rescaled_to_exact(finer).expect("exact by construction");
        prop_assert_eq!(there, t);
        prop_assert_eq!(there.rescaled_to_exact(rate), Some(t));
    }

    /// `rescaled_to` is exact exactly when the value is representable at the
    /// target rate, and rounds otherwise.
    #[test]
    fn rescale_is_exact_iff_representable(t in any_time(), rate in any_rate()) {
        let representable = t.is_exactly_representable_at(rate);
        let exact = t.rescaled_to_exact(rate);
        let Some(rounded) = t.checked_rescaled_to(rate) else {
            // Overflow is reported, never silently wrapped.
            prop_assert!(exact.is_none());
            return Ok(());
        };
        prop_assert_eq!(representable, exact.is_some());
        prop_assert_eq!(rounded.rate(), rate);
        prop_assert_eq!(representable, rounded == t);
        if let Some(exact) = exact {
            prop_assert_eq!(rounded.value(), exact.value());
        }
    }

    /// Every rounding mode lands on one of the two units bracketing the exact
    /// value, and floor never exceeds ceil.
    #[test]
    fn rounding_modes_bracket_the_exact_value(t in any_time(), rate in any_rate()) {
        let modes = [Rounding::Floor, Rounding::Ceil, Rounding::Trunc, Rounding::Nearest];
        let mut values = Vec::with_capacity(modes.len());
        for mode in modes {
            let Some(v) = t.checked_rescaled_to_rounding(rate, mode) else {
                return Ok(());
            };
            values.push(v.value());
        }
        let (floor, ceil) = (values[0], values[1]);
        prop_assert!(floor <= ceil, "floor {} above ceil {}", floor, ceil);
        prop_assert!(ceil - floor <= 1, "bracket wider than one unit");
        for (mode, value) in modes.iter().zip(&values) {
            prop_assert!(
                (floor..=ceil).contains(value),
                "{:?} left the bracket [{}, {}]", mode, floor, ceil
            );
        }
        if t.is_exactly_representable_at(rate) {
            prop_assert_eq!(floor, ceil, "an exact value has nothing to round");
        }
        // Truncation is towards zero, so it never exceeds either bound in
        // magnitude.
        prop_assert!(values[2].abs() <= floor.abs().max(ceil.abs()));
    }

    /// Addition and subtraction invert each other, whatever the rates.
    #[test]
    fn add_then_sub_is_identity(a in any_time(), b in any_time()) {
        if let Some(sum) = a.checked_add(b) {
            prop_assert_eq!(sum.checked_sub(b), Some(a));
            prop_assert_eq!(sum.checked_sub(a), Some(b));
        }
        if let Some(diff) = a.checked_sub(b) {
            prop_assert_eq!(diff.checked_add(b), Some(a));
        }
    }

    /// Addition is commutative with zero as its identity; negation is an
    /// involution and turns subtraction into addition.
    #[test]
    fn addition_is_an_abelian_group_where_defined(
        a in any_time(),
        b in any_time(),
        rate in any_rate(),
    ) {
        if let (Some(ab), Some(ba)) = (a.checked_add(b), b.checked_add(a)) {
            prop_assert_eq!(ab, ba);
        }
        // Zero is the identity; at a rate with no common rate inside `u32` the
        // sum is reported as `None` instead of being approximated.
        prop_assert_eq!(a.checked_add(RationalTime::zero(a.rate())), Some(a));
        if let Some(sum) = a.checked_add(RationalTime::zero(rate)) {
            prop_assert_eq!(sum, a);
        }
        prop_assert_eq!(a.checked_sub(a).map(RationalTime::is_zero), Some(true));
        let neg = a.checked_neg().expect("bounded values negate");
        prop_assert_eq!(neg.checked_neg(), Some(a));
        prop_assert_eq!(a.checked_add(neg).map(RationalTime::is_zero), Some(true));
        let minus = a.checked_sub(b);
        let plus_neg = b.checked_neg().and_then(|n| a.checked_add(n));
        if let (Some(minus), Some(plus_neg)) = (minus, plus_neg) {
            prop_assert_eq!(minus, plus_neg);
        }
    }

    /// Addition is associative when every intermediate result exists.
    #[test]
    fn addition_is_associative(
        a in supported_time(),
        b in supported_time(),
        c in supported_time(),
    ) {
        let left = a.checked_add(b).and_then(|ab| ab.checked_add(c));
        let right = b.checked_add(c).and_then(|bc| a.checked_add(bc));
        if let (Some(left), Some(right)) = (left, right) {
            prop_assert_eq!(left, right);
        }
    }

    /// Ordering is a total order that agrees with the independent model, and
    /// equality agrees with `Hash`.
    #[test]
    fn ordering_is_total(a in any_time(), b in any_time(), c in any_time()) {
        use core::cmp::Ordering;

        // Reflexive, and consistent with `Eq`.
        prop_assert_eq!(a.cmp(&a), Ordering::Equal);
        prop_assert_eq!(a, a);

        // Trichotomy: exactly one of <, ==, > holds.
        let ord = a.cmp(&b);
        prop_assert_eq!(u8::from(a < b) + u8::from(a == b) + u8::from(a > b), 1);
        prop_assert_eq!(a.partial_cmp(&b), Some(ord));

        // Antisymmetric, and matching the model.
        prop_assert_eq!(b.cmp(&a), ord.reverse());
        prop_assert_eq!(ord, model_cmp(a, b));

        // Transitive, checked over the sorted triple so the hypothesis holds.
        let mut sorted = [a, b, c];
        sorted.sort_unstable();
        prop_assert!(sorted[0] <= sorted[1] && sorted[1] <= sorted[2]);
        prop_assert!(sorted[0] <= sorted[2]);

        // Equality implies equal hashes.
        if a == b {
            prop_assert_eq!(hash_of(a), hash_of(b));
        }
    }

    /// Ordering is preserved by exact rescaling and reversed by negation.
    #[test]
    fn ordering_survives_rescaling_and_negation(
        a in any_time(),
        b in any_time(),
        rate in any_rate(),
    ) {
        let ord = a.cmp(&b);
        if let (Some(ra), Some(rb)) = (a.rescaled_to_exact(rate), b.rescaled_to_exact(rate)) {
            prop_assert_eq!(ra.cmp(&rb), ord);
            prop_assert_eq!(ra.value().cmp(&rb.value()), ord);
        }
        let na = a.checked_neg().expect("bounded values negate");
        let nb = b.checked_neg().expect("bounded values negate");
        prop_assert_eq!(na.cmp(&nb), ord.reverse());
        prop_assert_eq!(na.is_negative(), !a.is_negative() && !a.is_zero());
    }

    /// The reduced seconds fraction is canonical: equal times share it,
    /// different times do not, and it keeps the sign with a positive
    /// denominator.
    #[test]
    fn seconds_fraction_is_canonical(a in any_time(), b in any_time()) {
        let (an, ad) = a.as_seconds_fraction();
        prop_assert!(ad > 0);
        prop_assert_eq!(an == 0, a.is_zero());
        prop_assert_eq!(an < 0, a.is_negative());
        if a == b {
            prop_assert_eq!(a.as_seconds_fraction(), b.as_seconds_fraction());
        } else {
            prop_assert_ne!(a.as_seconds_fraction(), b.as_seconds_fraction());
        }
    }

    /// A range holds exactly the instants between its bounds, and clamping
    /// lands inside `[start, end]`.
    #[test]
    fn range_containment_matches_its_bounds(range in any_range(), t in any_time()) {
        let (start, end) = (range.start(), range.end_exclusive());
        prop_assert!(start <= end);
        prop_assert_eq!(range.contains(t), start <= t && t < end);
        prop_assert_eq!(range.is_empty(), start == end);
        prop_assert!(!range.contains(end));

        let clamped = range.clamp(t);
        prop_assert!(start <= clamped && clamped <= end);
        if range.contains(t) {
            prop_assert_eq!(clamped, t);
        }
        prop_assert_eq!(range.clamp(clamped), clamped);
        prop_assert!(range.contains_range(range));
    }

    /// Intersection is the commutative greatest lower bound of two ranges, and
    /// it exists exactly when the ranges overlap.
    #[test]
    fn intersection_agrees_with_overlap(a in supported_range(), b in supported_range()) {
        let overlap = a.overlaps(b);
        prop_assert_eq!(overlap, b.overlaps(a));
        let hit = a.intersection(b);
        prop_assert_eq!(hit.is_some(), overlap);
        if let (Some(hit), Some(flipped)) = (hit, b.intersection(a)) {
            prop_assert_eq!(hit.start(), flipped.start());
            prop_assert_eq!(hit.duration(), flipped.duration());
            prop_assert!(a.contains_range(hit) && b.contains_range(hit));
            prop_assert_eq!(hit.start(), a.start().max(b.start()));
            prop_assert_eq!(hit.end_exclusive(), a.end_exclusive().min(b.end_exclusive()));
        }
    }

    /// Shifting a range moves its bounds and keeps its length.
    #[test]
    fn shifting_a_range_preserves_its_duration(range in any_range(), offset in any_time()) {
        if let Some(shifted) = range.shifted_by(offset) {
            prop_assert_eq!(shifted.duration(), range.duration());
            let moved = range.start().checked_add(offset).expect("the shift exists");
            prop_assert_eq!(shifted.start(), moved);
            let back_by = offset.checked_neg().expect("bounded values negate");
            if let Some(back) = shifted.shifted_by(back_by) {
                prop_assert_eq!(back.start(), range.start());
                prop_assert_eq!(back.duration(), range.duration());
            }
        }
    }

    /// Frame numbers, labels and their text form all describe the same frame,
    /// at drop-frame and non-drop-frame rates alike.
    #[test]
    fn timecode_round_trips_through_label_and_text(
        rate in any_timecode_rate(),
        offset in -3_000_000_i64..3_000_000,
    ) {
        let day = rate.frames_per_24h();
        prop_assert!(day > 0);
        let frame = offset.rem_euclid(day);
        let tc = Timecode::from_frame_number(frame, rate);
        prop_assert_eq!(tc.to_frame_number(), frame);
        prop_assert!(tc.hours() < 24 && tc.minutes() < 60 && tc.seconds() < 60);
        prop_assert!(tc.frames() < rate.nominal_fps());

        // Text round-trips, and wrapping is by whole days.
        let text = tc.to_string();
        prop_assert_eq!(Timecode::parse(&text, rate).ok(), Some(tc));
        prop_assert_eq!(Timecode::from_frame_number(offset, rate), tc);
        prop_assert_eq!(Timecode::from_frame_number(frame + day, rate), tc);

        // And the rational time it names labels the same frame again.
        let time = tc.to_rational_time();
        prop_assert_eq!(time.value(), frame);
        prop_assert_eq!(Timecode::from_rational_time(time, rate).ok(), Some(tc));
    }

    /// Consecutive frame numbers get consecutive, distinct labels, and a
    /// drop-frame label is never one of the labels the rate skips.
    #[test]
    fn timecode_labels_advance_monotonically(
        rate in any_timecode_rate(),
        offset in -3_000_000_i64..3_000_000,
    ) {
        let day = rate.frames_per_24h();
        let frame = offset.rem_euclid(day);
        let next = (frame + 1).rem_euclid(day);
        let here = Timecode::from_frame_number(frame, rate);
        let there = Timecode::from_frame_number(next, rate);
        prop_assert_ne!(here, there);
        prop_assert_eq!(there.to_frame_number(), next);
        if rate.is_drop_frame() {
            let dropped = !here.minutes().is_multiple_of(10)
                && here.seconds() == 0
                && here.frames() < rate.nominal_fps() / 15;
            prop_assert!(!dropped, "{} is a label drop-frame counting skips", here);
            prop_assert!(here.to_string().contains(';'));
        } else {
            prop_assert!(!here.to_string().contains(';'));
        }
    }

    /// A time at the timecode rate keeps its frame number through the label,
    /// modulo the 24-hour wrap.
    #[test]
    fn timecode_from_rational_time_wraps_by_day(
        rate in any_timecode_rate(),
        value in -3_000_000_i64..3_000_000,
    ) {
        let time = RationalTime::new(value, rate.rate());
        let tc = Timecode::from_rational_time(time, rate).expect("the value is small enough");
        prop_assert_eq!(tc.to_frame_number(), value.rem_euclid(rate.frames_per_24h()));
    }
}
