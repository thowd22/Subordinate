//! Exact timeline arithmetic for plugins.
//!
//! Every instant Subordinate exchanges is a tick count and a fractional rate,
//! never a number of seconds (docs/PLAN.md §2). The generated
//! [`RationalTime`] record carries both, but bare records make even trivial
//! work verbose: constructing one at 23.976 fps means typing `24000/1001`, and
//! comparing two at different rates means cross-multiplying by hand — which is
//! precisely where a plugin author reaches for `as f64` and introduces drift.
//!
//! This module is the alternative. [`rate`] names the broadcast rates,
//! [`frames`] and [`seconds`] build times, [`compare`] and [`add`] do arithmetic
//! that is exact across mixed rates, and [`range`] builds a span. Nothing here
//! converts to a float, and every operation that can overflow returns [`None`]
//! rather than wrapping.
//!
//! Comparison widens to [`i128`] before cross-multiplying: the largest product
//! two `i64`-by-`u32` operands can reach is far inside `i128`, so a comparison
//! is exact for every value the boundary can carry.
//!
//! ```
//! use subordinate_sdk::time::{compare, frames, rate};
//! use std::cmp::Ordering;
//!
//! // One frame at 24 fps is exactly two at 48 fps.
//! let a = frames(1, rate::FPS_24);
//! let b = frames(2, rate::FPS_48);
//! assert_eq!(compare(a, b), Some(Ordering::Equal));
//! ```

use std::cmp::Ordering;

use crate::bindings::subordinate::plugin::types::{Rational, RationalTime, TimeRange};

/// The rates a plugin is likely to need, exact as fractions.
///
/// NTSC rates are `n/1001` and are never rounded: 23.976 fps is `24000/1001`,
/// not `23.976`.
pub mod rate {
    use super::Rational;

    /// Builds a rate of `numerator / denominator` ticks per second.
    ///
    /// The fraction is left unreduced, because a rate is compared by value on
    /// the wire: `30000/1001` and `60000/2002` mean the same thing but are not
    /// the same [`Rational`].
    ///
    /// # Panics
    ///
    /// Panics when either part is zero. Both are documented as non-zero in the
    /// WIT, and a zero rate is a programming error rather than a runtime
    /// condition a plugin should branch on.
    #[must_use]
    pub const fn new(numerator: u32, denominator: u32) -> Rational {
        assert!(
            numerator != 0 && denominator != 0,
            "a rate has a non-zero numerator and denominator"
        );
        Rational {
            numerator,
            denominator,
        }
    }

    /// One tick per second: the rate whole seconds are counted at.
    pub const SECONDS: Rational = new(1, 1);
    /// 23.976 fps, exactly `24000/1001`.
    pub const FPS_23_976: Rational = new(24_000, 1_001);
    /// 24 fps.
    pub const FPS_24: Rational = new(24, 1);
    /// 25 fps.
    pub const FPS_25: Rational = new(25, 1);
    /// 29.97 fps, exactly `30000/1001`.
    pub const FPS_29_97: Rational = new(30_000, 1_001);
    /// 30 fps.
    pub const FPS_30: Rational = new(30, 1);
    /// 48 fps.
    pub const FPS_48: Rational = new(48, 1);
    /// 50 fps.
    pub const FPS_50: Rational = new(50, 1);
    /// 59.94 fps, exactly `60000/1001`.
    pub const FPS_59_94: Rational = new(60_000, 1_001);
    /// 60 fps.
    pub const FPS_60: Rational = new(60, 1);
}

/// An instant of `count` ticks at `rate`.
#[must_use]
pub const fn frames(count: i64, rate: Rational) -> RationalTime {
    RationalTime { value: count, rate }
}

/// A whole number of seconds, counted at one tick per second.
#[must_use]
pub const fn seconds(count: i64) -> RationalTime {
    frames(count, rate::SECONDS)
}

/// Zero at `rate`.
#[must_use]
pub const fn zero(rate: Rational) -> RationalTime {
    frames(0, rate)
}

/// The half-open span `[start, start + duration)`.
#[must_use]
pub const fn range(start: RationalTime, duration: RationalTime) -> TimeRange {
    TimeRange { start, duration }
}

/// `time` in seconds, as an exact fraction `(numerator, denominator)`.
///
/// A plugin that must show a time to a human formats this fraction itself; the
/// SDK never hands out a float, because a float is where NTSC drift starts.
#[must_use]
pub fn as_seconds_fraction(time: RationalTime) -> (i128, i128) {
    (
        i128::from(time.value) * i128::from(time.rate.denominator),
        i128::from(time.rate.numerator),
    )
}

/// Orders two instants exactly, whatever rates they are expressed at.
///
/// Returns [`None`] only when a rate is degenerate — a zero numerator or
/// denominator, which the WIT forbids but a malformed host response could still
/// carry — because there is then no instant to compare.
#[must_use]
pub fn compare(left: RationalTime, right: RationalTime) -> Option<Ordering> {
    if left.rate.numerator == 0
        || left.rate.denominator == 0
        || right.rate.numerator == 0
        || right.rate.denominator == 0
    {
        return None;
    }
    // left.value * left.rate.denominator / left.rate.numerator, compared with
    // the same for the right, cross-multiplied so nothing divides.
    let (left_seconds, left_per_second) = as_seconds_fraction(left);
    let (right_seconds, right_per_second) = as_seconds_fraction(right);
    Some((left_seconds * right_per_second).cmp(&(right_seconds * left_per_second)))
}

/// Whether two instants name the same moment, at whatever rates.
#[must_use]
pub fn equals(left: RationalTime, right: RationalTime) -> bool {
    compare(left, right) == Some(Ordering::Equal)
}

/// `left + right`, expressed at `left`'s rate.
///
/// Returns [`None`] when the sum is not representable at that rate — either the
/// tick count overflows or `right` does not land on a whole tick of `left`'s
/// rate. Refusing an inexact sum is deliberate: silently rounding a frame
/// boundary is the bug this whole type exists to prevent.
#[must_use]
pub fn add(left: RationalTime, right: RationalTime) -> Option<RationalTime> {
    let ticks = rescale_ticks(right, left.rate)?;
    Some(frames(left.value.checked_add(ticks)?, left.rate))
}

/// `left - right`, expressed at `left`'s rate.
///
/// Returns [`None`] on the same terms as [`add`].
#[must_use]
pub fn sub(left: RationalTime, right: RationalTime) -> Option<RationalTime> {
    let ticks = rescale_ticks(right, left.rate)?;
    Some(frames(left.value.checked_sub(ticks)?, left.rate))
}

/// `time` expressed at `rate`, or [`None`] when it does not land on a whole
/// tick of that rate.
#[must_use]
pub fn rescale(time: RationalTime, rate: Rational) -> Option<RationalTime> {
    Some(frames(rescale_ticks(time, rate)?, rate))
}

/// The negation of `time`, or [`None`] when the tick count is `i64::MIN`.
#[must_use]
pub fn neg(time: RationalTime) -> Option<RationalTime> {
    Some(frames(time.value.checked_neg()?, time.rate))
}

/// The end of a span, exclusive, expressed at the start's rate.
#[must_use]
pub fn end_exclusive(span: TimeRange) -> Option<RationalTime> {
    add(span.start, span.duration)
}

/// Whether `span` contains `time`, comparing exactly across rates.
#[must_use]
pub fn contains(span: TimeRange, time: RationalTime) -> bool {
    let Some(end) = end_exclusive(span) else {
        return false;
    };
    compare(span.start, time).is_some_and(Ordering::is_le)
        && compare(time, end).is_some_and(Ordering::is_lt)
}

/// `time` as a tick count at `rate`, exactly or not at all.
///
/// `value * (denominator / numerator)` seconds becomes
/// `value * denominator * rate.numerator / (numerator * rate.denominator)`
/// ticks; the division has to be exact.
fn rescale_ticks(time: RationalTime, rate: Rational) -> Option<i64> {
    if time.rate.numerator == 0 || time.rate.denominator == 0 || rate.denominator == 0 {
        return None;
    }
    if time.rate.numerator == rate.numerator && time.rate.denominator == rate.denominator {
        return Some(time.value);
    }
    let numerator =
        i128::from(time.value) * i128::from(time.rate.denominator) * i128::from(rate.numerator);
    let denominator = i128::from(time.rate.numerator) * i128::from(rate.denominator);
    if denominator == 0 || numerator % denominator != 0 {
        return None;
    }
    i64::try_from(numerator / denominator).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        add, compare, contains, end_exclusive, equals, frames, neg, range, rate, rescale, seconds,
        sub, zero,
    };
    use std::cmp::Ordering;

    #[test]
    fn instants_compare_exactly_across_rates() {
        assert_eq!(
            compare(frames(1, rate::FPS_24), frames(2, rate::FPS_48)),
            Some(Ordering::Equal)
        );
        assert_eq!(
            compare(frames(1, rate::FPS_24), frames(1, rate::FPS_48)),
            Some(Ordering::Greater)
        );
        assert!(equals(seconds(1), frames(24, rate::FPS_24)));
    }

    #[test]
    fn ntsc_rates_are_not_rounded_to_their_integer_neighbours() {
        // 24 frames at 23.976 fps is 1.001 seconds, strictly more than a second.
        assert_eq!(
            compare(frames(24, rate::FPS_23_976), seconds(1)),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn arithmetic_stays_at_the_left_rate_and_refuses_inexact_sums() {
        let sum = add(frames(10, rate::FPS_24), frames(2, rate::FPS_48)).unwrap();
        assert_eq!(sum.value, 11);
        assert_eq!(sum.rate.numerator, 24);
        // One frame at 48 fps is half a frame at 24 fps: not representable.
        assert!(add(frames(10, rate::FPS_24), frames(1, rate::FPS_48)).is_none());
        assert_eq!(
            sub(frames(10, rate::FPS_24), frames(4, rate::FPS_48))
                .unwrap()
                .value,
            8
        );
    }

    #[test]
    fn rescaling_is_exact_or_absent() {
        assert_eq!(
            rescale(frames(1, rate::FPS_24), rate::FPS_48)
                .unwrap()
                .value,
            2
        );
        assert!(rescale(frames(1, rate::FPS_48), rate::FPS_24).is_none());
        assert_eq!(
            rescale(frames(3, rate::FPS_24), rate::FPS_24)
                .unwrap()
                .value,
            3
        );
    }

    #[test]
    fn a_span_is_half_open() {
        let span = range(frames(10, rate::FPS_24), frames(5, rate::FPS_24));
        assert_eq!(end_exclusive(span).unwrap().value, 15);
        assert!(contains(span, frames(10, rate::FPS_24)));
        assert!(contains(span, frames(28, rate::FPS_48)));
        assert!(!contains(span, frames(15, rate::FPS_24)));
        assert!(!contains(span, frames(9, rate::FPS_24)));
    }

    #[test]
    fn overflow_and_degenerate_rates_are_absent_rather_than_wrong() {
        assert!(add(frames(i64::MAX, rate::FPS_24), frames(1, rate::FPS_24)).is_none());
        assert!(neg(frames(i64::MIN, rate::FPS_24)).is_none());
        assert_eq!(neg(frames(5, rate::FPS_25)).unwrap().value, -5);
        assert_eq!(zero(rate::FPS_25).value, 0);
    }
}
