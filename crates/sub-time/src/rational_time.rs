//! Integer time at an exact rational rate.

use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::ops::{Add, Neg, Sub};

use crate::rational::{Rational, gcd_u128};

/// How [`RationalTime::rescaled_to_rounding`] resolves a value that is not
/// exactly representable at the target rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rounding {
    /// Round to the closest representable value, ties away from zero.
    ///
    /// This is the default used by [`RationalTime::rescaled_to`].
    Nearest,
    /// Round towards negative infinity.
    Floor,
    /// Round towards positive infinity.
    Ceil,
    /// Round towards zero.
    Trunc,
}

/// An exact instant or duration: an integer `value` of units at a rational
/// `rate`.
///
/// The wall-clock length is `value * rate.denominator() / rate.numerator()`
/// seconds. All arithmetic here is integer arithmetic; no operation on this
/// type uses a float, so edits never drift (see docs/PLAN.md §5.1).
///
/// Equality and ordering compare the *duration in seconds*, not the raw
/// representation: `1` frame at 24 fps equals `2` frames at 48 fps. [`Hash`]
/// agrees with that equality.
#[derive(Debug, Clone, Copy)]
pub struct RationalTime {
    value: i64,
    rate: Rational,
}

impl RationalTime {
    /// Creates a time of `value` units at `rate`.
    pub const fn new(value: i64, rate: Rational) -> Self {
        Self { value, rate }
    }

    /// Creates a time of `frames` frames at `rate`. An alias for [`Self::new`]
    /// that reads better at call sites dealing with frame counts.
    pub const fn from_frames(frames: i64, rate: Rational) -> Self {
        Self::new(frames, rate)
    }

    /// Creates a whole number of seconds, at a rate of one unit per second.
    pub const fn from_seconds(seconds: i64) -> Self {
        Self::new(seconds, Rational::ONE)
    }

    /// Zero at `rate`.
    pub const fn zero(rate: Rational) -> Self {
        Self::new(0, rate)
    }

    /// The integer unit count.
    pub const fn value(self) -> i64 {
        self.value
    }

    /// The rate the unit count is expressed in.
    pub const fn rate(self) -> Rational {
        self.rate
    }

    /// True if this is zero-length, whatever the rate.
    pub const fn is_zero(self) -> bool {
        self.value == 0
    }

    /// True if this is a negative duration or an instant before zero.
    pub const fn is_negative(self) -> bool {
        self.value < 0
    }

    /// The length in seconds as an exact reduced fraction `(numerator,
    /// denominator)` with a strictly positive denominator.
    pub fn as_seconds_fraction(self) -> (i128, i128) {
        let num = i128::from(self.value) * i128::from(self.rate.denominator());
        let den = i128::from(self.rate.numerator());
        let g = gcd_u128(num.unsigned_abs(), den.unsigned_abs());
        if g == 0 {
            // Only reachable when the value is zero; the rate is never zero.
            return (0, 1);
        }
        let g = i128::try_from(g).unwrap_or(1);
        (num / g, den / g)
    }

    /// Numerator and (positive) denominator of this time expressed at `rate`,
    /// before any rounding.
    fn parts_at(self, rate: Rational) -> (i128, i128) {
        let num = i128::from(self.value)
            * i128::from(rate.numerator())
            * i128::from(self.rate.denominator());
        let den = i128::from(rate.denominator()) * i128::from(self.rate.numerator());
        (num, den)
    }

    /// True if this time is representable at `rate` with no rounding.
    pub fn is_exactly_representable_at(self, rate: Rational) -> bool {
        let (num, den) = self.parts_at(rate);
        num % den == 0
    }

    /// Converts to `rate` exactly, or returns `None` if the value is not
    /// representable at `rate` (or would overflow `i64`).
    pub fn rescaled_to_exact(self, rate: Rational) -> Option<Self> {
        let (num, den) = self.parts_at(rate);
        if num % den != 0 {
            return None;
        }
        Some(Self::new(i64::try_from(num / den).ok()?, rate))
    }

    /// Converts to `rate`.
    ///
    /// The result is exact whenever the value is representable at `rate` (for
    /// example 24 fps to 48 fps, or any conversion between `24000/1001` and
    /// `120000/1001`). Otherwise it rounds to the nearest representable unit,
    /// ties away from zero; use [`Self::rescaled_to_rounding`] to pick another
    /// mode or [`Self::rescaled_to_exact`] to require exactness.
    ///
    /// # Panics
    ///
    /// Panics if the result does not fit in an `i64`. Use
    /// [`Self::checked_rescaled_to`] to handle that case.
    #[must_use]
    pub fn rescaled_to(self, rate: Rational) -> Self {
        self.rescaled_to_rounding(rate, Rounding::Nearest)
    }

    /// Converts to `rate`, resolving inexact conversions with `rounding`.
    ///
    /// # Panics
    ///
    /// Panics if the result does not fit in an `i64`.
    #[must_use]
    pub fn rescaled_to_rounding(self, rate: Rational, rounding: Rounding) -> Self {
        self.checked_rescaled_to_rounding(rate, rounding)
            .expect("rescaled RationalTime overflowed i64")
    }

    /// Like [`Self::rescaled_to`] but returns `None` instead of panicking on
    /// `i64` overflow.
    pub fn checked_rescaled_to(self, rate: Rational) -> Option<Self> {
        self.checked_rescaled_to_rounding(rate, Rounding::Nearest)
    }

    /// Like [`Self::rescaled_to_rounding`] but returns `None` instead of
    /// panicking on `i64` overflow.
    pub fn checked_rescaled_to_rounding(self, rate: Rational, rounding: Rounding) -> Option<Self> {
        let (num, den) = self.parts_at(rate);
        let value = div_round(num, den, rounding);
        Some(Self::new(i64::try_from(value).ok()?, rate))
    }

    /// Adds two times exactly, at the smallest rate that represents both.
    ///
    /// Returns `None` if no such rate is representable or the result overflows
    /// `i64`. The result carries `self.rate()` when both operands share it.
    pub fn checked_add(self, other: Self) -> Option<Self> {
        let (a, b) = self.aligned(other)?;
        Some(Self::new(a.value.checked_add(b.value)?, a.rate))
    }

    /// Subtracts `other` from `self` exactly, at the smallest rate that
    /// represents both.
    ///
    /// Returns `None` if no such rate is representable or the result overflows
    /// `i64`.
    pub fn checked_sub(self, other: Self) -> Option<Self> {
        let (a, b) = self.aligned(other)?;
        Some(Self::new(a.value.checked_sub(b.value)?, a.rate))
    }

    /// Negates the value, returning `None` on `i64` overflow.
    pub fn checked_neg(self) -> Option<Self> {
        Some(Self::new(self.value.checked_neg()?, self.rate))
    }

    /// The absolute duration, returning `None` on `i64` overflow.
    pub fn checked_abs(self) -> Option<Self> {
        Some(Self::new(self.value.checked_abs()?, self.rate))
    }

    /// Rewrites both operands at a rate that represents each of them exactly.
    fn aligned(self, other: Self) -> Option<(Self, Self)> {
        if self.rate == other.rate {
            return Some((self, other));
        }
        let rate = self.rate.common_rate(other.rate)?;
        Some((
            self.rescaled_to_exact(rate)?,
            other.rescaled_to_exact(rate)?,
        ))
    }
}

/// Divides `num` by a strictly positive `den` under `rounding`.
fn div_round(num: i128, den: i128, rounding: Rounding) -> i128 {
    debug_assert!(den > 0, "rate denominators are always positive");
    let q = num.div_euclid(den);
    let r = num.rem_euclid(den);
    if r == 0 {
        return q;
    }
    match rounding {
        // `div_euclid` already floors for a positive divisor.
        Rounding::Floor => q,
        Rounding::Ceil => q + 1,
        Rounding::Trunc => {
            if num < 0 {
                q + 1
            } else {
                q
            }
        }
        Rounding::Nearest => {
            let twice = r * 2;
            match twice.cmp(&den) {
                Ordering::Greater => q + 1,
                Ordering::Less => q,
                // A tie: round away from zero.
                Ordering::Equal => {
                    if num < 0 {
                        q
                    } else {
                        q + 1
                    }
                }
            }
        }
    }
}

impl PartialEq for RationalTime {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for RationalTime {}

impl PartialOrd for RationalTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RationalTime {
    /// Compares wall-clock lengths exactly, by cross-multiplying in `i128`.
    fn cmp(&self, other: &Self) -> Ordering {
        // seconds = value * denominator / numerator, and numerators are
        // positive, so cross-multiplication preserves the ordering.
        let lhs = i128::from(self.value)
            * i128::from(self.rate.denominator())
            * i128::from(other.rate.numerator());
        let rhs = i128::from(other.value)
            * i128::from(other.rate.denominator())
            * i128::from(self.rate.numerator());
        lhs.cmp(&rhs)
    }
}

impl Hash for RationalTime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_seconds_fraction().hash(state);
    }
}

impl Add for RationalTime {
    type Output = Self;

    /// # Panics
    ///
    /// Panics on `i64` overflow or when no common rate is representable; use
    /// [`RationalTime::checked_add`] to handle those cases.
    fn add(self, rhs: Self) -> Self {
        self.checked_add(rhs)
            .expect("RationalTime addition overflowed")
    }
}

impl Sub for RationalTime {
    type Output = Self;

    /// # Panics
    ///
    /// Panics on `i64` overflow or when no common rate is representable; use
    /// [`RationalTime::checked_sub`] to handle those cases.
    fn sub(self, rhs: Self) -> Self {
        self.checked_sub(rhs)
            .expect("RationalTime subtraction overflowed")
    }
}

impl Neg for RationalTime {
    type Output = Self;

    /// # Panics
    ///
    /// Panics on `i64` overflow (negating `i64::MIN`).
    fn neg(self) -> Self {
        self.checked_neg()
            .expect("RationalTime negation overflowed")
    }
}

impl fmt::Display for RationalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.value, self.rate)
    }
}

#[cfg(test)]
mod tests {
    use super::{RationalTime, Rounding};
    use crate::rational::Rational;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    const NTSC_24: Rational = Rational::FPS_23_976;
    const FPS_48: Rational = match Rational::new(48, 1) {
        Some(r) => r,
        None => unreachable!(),
    };
    const NTSC_30: Rational = Rational::FPS_29_97;

    fn hash_of(t: RationalTime) -> u64 {
        let mut h = DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    }

    #[test]
    fn equality_compares_duration_not_representation() {
        let a = RationalTime::new(1, Rational::FPS_24);
        let b = RationalTime::new(2, Rational::new(48, 1).unwrap());
        assert_eq!(a, b);
        assert_eq!(hash_of(a), hash_of(b));
        assert_ne!(a, RationalTime::new(1, Rational::FPS_25));
    }

    #[test]
    fn ordering_and_min_max_across_rates() {
        let a = RationalTime::new(24, Rational::FPS_24); // 1 second
        let b = RationalTime::new(24, NTSC_24); // slightly more than 1 second
        assert!(b > a);
        assert_eq!(a.min(b), a);
        assert_eq!(a.max(b), b);
        assert_eq!(
            [b, a].iter().copied().min().unwrap(),
            RationalTime::new(24, Rational::FPS_24)
        );
    }

    #[test]
    fn ordering_handles_negatives() {
        let a = RationalTime::new(-1, Rational::FPS_24);
        let b = RationalTime::new(0, NTSC_30);
        assert!(a < b);
        assert_eq!(-a, RationalTime::new(1, Rational::FPS_24));
    }

    #[test]
    fn same_rate_addition_keeps_the_rate() {
        let a = RationalTime::new(10, NTSC_24);
        let b = RationalTime::new(5, NTSC_24);
        let sum = a + b;
        assert_eq!(sum.value(), 15);
        assert_eq!(sum.rate(), NTSC_24);
        assert_eq!((sum - b).value(), 10);
    }

    #[test]
    fn mixed_ntsc_rate_addition_is_exact() {
        // 24000/1001 and 30000/1001 share the exact rate 120000/1001.
        let a = RationalTime::new(1, NTSC_24);
        let b = RationalTime::new(1, NTSC_30);
        let sum = a + b;
        assert_eq!(sum.rate(), Rational::new(120_000, 1001).unwrap());
        assert_eq!(sum.value(), 5 + 4);
        // The sum is exactly one frame of each input added together.
        assert_eq!(
            sum,
            RationalTime::new(9, Rational::new(120_000, 1001).unwrap())
        );
        assert_eq!(sum - b, a);
        assert_eq!(sum - a, b);
    }

    #[test]
    fn mixed_ntsc_and_integral_rate_addition_is_exact() {
        let a = RationalTime::new(24000, NTSC_24); // exactly 1001 seconds
        let b = RationalTime::new(48, Rational::FPS_24); // exactly 2 seconds
        let sum = a + b;
        assert_eq!(sum, RationalTime::from_seconds(1003));
        assert_eq!(sum.as_seconds_fraction(), (1003, 1));
    }

    #[test]
    fn a_thousand_ntsc_additions_do_not_drift() {
        let step = RationalTime::new(1, NTSC_24);
        let mut acc = RationalTime::zero(NTSC_24);
        for _ in 0..1000 {
            acc = acc + step;
        }
        assert_eq!(acc, RationalTime::new(1000, NTSC_24));
        assert_eq!(acc.as_seconds_fraction(), (1001, 24));
    }

    #[test]
    fn rescale_is_exact_when_representable() {
        let t = RationalTime::new(12, Rational::FPS_24);
        assert!(t.is_exactly_representable_at(FPS_48));
        assert_eq!(t.rescaled_to_exact(FPS_48).unwrap().value(), 24);
        assert_eq!(t.rescaled_to(FPS_48).value(), 24);

        let ntsc = RationalTime::new(7, NTSC_24);
        let fine = Rational::new(120_000, 1001).unwrap();
        assert_eq!(ntsc.rescaled_to_exact(fine).unwrap().value(), 35);
        assert_eq!(ntsc.rescaled_to(fine), ntsc);
    }

    #[test]
    fn rescale_rounds_when_not_representable() {
        // 1 frame at 24 fps is 25/24 frames at 25 fps.
        let t = RationalTime::new(1, Rational::FPS_24);
        assert!(!t.is_exactly_representable_at(Rational::FPS_25));
        assert!(t.rescaled_to_exact(Rational::FPS_25).is_none());
        assert_eq!(t.rescaled_to(Rational::FPS_25).value(), 1);
        assert_eq!(
            t.rescaled_to_rounding(Rational::FPS_25, Rounding::Ceil)
                .value(),
            2
        );
        assert_eq!(
            t.rescaled_to_rounding(Rational::FPS_25, Rounding::Floor)
                .value(),
            1
        );
        assert_eq!(
            t.rescaled_to_rounding(Rational::FPS_25, Rounding::Trunc)
                .value(),
            1
        );
    }

    #[test]
    fn rescale_rounding_modes_on_negatives_and_ties() {
        // 1 frame at 2 fps is exactly 1.5 frames at 3 fps: a tie.
        let two = Rational::from_integer(2).unwrap();
        let three = Rational::from_integer(3).unwrap();
        let pos = RationalTime::new(1, two);
        let neg = RationalTime::new(-1, two);
        assert_eq!(
            pos.rescaled_to(three).value(),
            2,
            "ties round away from zero"
        );
        assert_eq!(
            neg.rescaled_to(three).value(),
            -2,
            "ties round away from zero"
        );
        assert_eq!(pos.rescaled_to_rounding(three, Rounding::Floor).value(), 1);
        assert_eq!(neg.rescaled_to_rounding(three, Rounding::Floor).value(), -2);
        assert_eq!(pos.rescaled_to_rounding(three, Rounding::Ceil).value(), 2);
        assert_eq!(neg.rescaled_to_rounding(three, Rounding::Ceil).value(), -1);
        assert_eq!(pos.rescaled_to_rounding(three, Rounding::Trunc).value(), 1);
        assert_eq!(neg.rescaled_to_rounding(three, Rounding::Trunc).value(), -1);
    }

    #[test]
    fn nearest_rounding_picks_the_closer_side() {
        // 1 frame at 24 fps = 25/24 at 25 fps -> 1; 23 frames -> 23.958 -> 24.
        assert_eq!(
            RationalTime::new(23, Rational::FPS_24)
                .rescaled_to(Rational::FPS_25)
                .value(),
            24
        );
        assert_eq!(
            RationalTime::new(-23, Rational::FPS_24)
                .rescaled_to(Rational::FPS_25)
                .value(),
            -24
        );
    }

    #[test]
    fn overflow_is_reported_not_wrapped() {
        let big = RationalTime::new(i64::MAX, Rational::FPS_24);
        assert!(
            big.checked_add(RationalTime::new(1, Rational::FPS_24))
                .is_none()
        );
        assert!(big.checked_rescaled_to(FPS_48).is_none());
        assert!(
            RationalTime::new(i64::MIN, Rational::FPS_24)
                .checked_neg()
                .is_none()
        );
        // No common rate exists within u32 for these two.
        let a = RationalTime::new(1, Rational::from_integer(4_000_000_007).unwrap());
        let b = RationalTime::new(1, Rational::from_integer(4_000_000_009).unwrap());
        assert!(a.checked_add(b).is_none());
    }

    #[test]
    fn seconds_fraction_is_reduced() {
        assert_eq!(RationalTime::new(0, NTSC_24).as_seconds_fraction(), (0, 1));
        assert_eq!(
            RationalTime::new(1, NTSC_24).as_seconds_fraction(),
            (1001, 24000)
        );
        assert_eq!(
            RationalTime::new(48, Rational::FPS_24).as_seconds_fraction(),
            (2, 1)
        );
        assert_eq!(
            RationalTime::new(-1, NTSC_30).as_seconds_fraction(),
            (-1001, 30000)
        );
    }

    #[test]
    fn display_shows_value_and_rate() {
        assert_eq!(RationalTime::new(5, NTSC_24).to_string(), "5@24000/1001");
        assert_eq!(RationalTime::new(5, Rational::FPS_24).to_string(), "5@24");
    }
}
