//! Exact time, and the one place a float is allowed to exist.
//!
//! Subordinate does all timeline math in exact rationals (docs/PLAN.md §5).
//! OTIO does not: a serialised `RationalTime.1` carries `rate` and `value` as
//! JSON numbers, so 23.976 fps reaches this plugin as
//! `23.976023976023978`. That conversion is the boundary and it lives here:
//!
//! - [`Rate`] is an exact reduced fraction. Nothing in the crate computes with
//!   [`Rate::as_f64`]; it is only ever handed to the serialiser.
//! - [`Rate::from_f64`] turns an OTIO rate back into a fraction, recognising
//!   the NTSC rates first — a rate that came from `30000/1001` must come back
//!   as `30000/1001` and not as some 15-digit approximation of it — and
//!   otherwise finding the simplest fraction that reproduces the number.
//! - [`Time::from_f64`] refuses a value that is not a whole number of ticks,
//!   because a fractional tick is not a position the model can hold.
//!
//! Everything else — durations, sums, rescaling — is integer arithmetic.

use crate::error::{Error, Result, codes};

/// The NTSC family: the rates whose exact form has a denominator of 1001.
///
/// They are checked before anything else because they are the rates that a
/// naive `f64` round-trip gets wrong, and they are common enough that getting
/// them wrong would show up as drift in a real edit.
const NTSC_NUMERATORS: [u32; 6] = [24_000, 30_000, 48_000, 60_000, 96_000, 120_000];

/// The largest denominator [`Rate::from_f64`] will invent for a rate it does
/// not recognise. Beyond this a rate is reported as unrepresentable rather
/// than approximated by something unwieldy.
const MAX_INVENTED_DENOMINATOR: u32 = 100_000;

/// Past 2^53 an `f64` no longer counts every whole number, so a tick count
/// larger than this is not one OTIO can have written exactly.
const EXACT_WHOLE_LIMIT: f64 = 9_007_199_254_740_992.0;

/// How close an `f64` rate must be to a candidate fraction to be considered
/// that fraction, relative to the rate itself.
const RATE_TOLERANCE: f64 = 1e-9;

/// An exact, strictly positive, reduced rate: `numerator / denominator` ticks
/// per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rate {
    numerator: u32,
    denominator: u32,
}

impl Rate {
    /// 24 fps, the rate an OTIO document falls back to when it declares none.
    pub const FPS_24: Self = Self {
        numerator: 24,
        denominator: 1,
    };

    /// Creates a reduced rate, or `None` if either part is zero.
    #[must_use]
    pub const fn new(numerator: u32, denominator: u32) -> Option<Self> {
        if numerator == 0 || denominator == 0 {
            return None;
        }
        let divisor = gcd(numerator, denominator);
        Some(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    /// The numerator of the reduced fraction.
    #[must_use]
    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    /// The denominator of the reduced fraction.
    #[must_use]
    pub const fn denominator(self) -> u32 {
        self.denominator
    }

    /// The rate as OTIO writes it. Only the serialiser calls this.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.numerator) / f64::from(self.denominator)
    }

    /// Recovers the exact rate an OTIO `rate` number stands for.
    ///
    /// # Errors
    ///
    /// Returns [`codes::INVALID_TIME`] when the number is not finite, is not
    /// strictly positive, or is not within [`RATE_TOLERANCE`] of any fraction
    /// with a denominator up to [`MAX_INVENTED_DENOMINATOR`].
    pub fn from_f64(rate: f64) -> Result<Self> {
        if !rate.is_finite() || rate <= 0.0 {
            return Err(
                Error::new(codes::INVALID_TIME, "rate must be finite and positive")
                    .with("rate", rate),
            );
        }

        for numerator in NTSC_NUMERATORS {
            let candidate = Self {
                numerator,
                denominator: 1001,
            };
            if close_enough(rate, candidate.as_f64()) {
                return Ok(candidate);
            }
        }

        // A whole number of ticks per second is the common case and needs no
        // search: 24, 25, 30, 48, 50, 60, and audio rates like 48000.
        let rounded = rate.round();
        if close_enough(rate, rounded) && rounded <= f64::from(u32::MAX) {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the value is finite, positive, whole and within u32 by the checks above"
            )]
            let numerator = rounded as u32;
            return Self::new(numerator, 1)
                .ok_or_else(|| Error::new(codes::INVALID_TIME, "rate must not be zero"));
        }

        simplest_fraction(rate).ok_or_else(|| {
            Error::new(
                codes::INVALID_TIME,
                "rate is not an exact fraction this plugin can represent",
            )
            .with("rate", rate)
        })
    }
}

/// An exact instant or duration: `value` ticks at `rate` ticks per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Time {
    value: i64,
    rate: Rate,
}

impl Time {
    /// A time of `value` ticks at `rate`.
    #[must_use]
    pub const fn new(value: i64, rate: Rate) -> Self {
        Self { value, rate }
    }

    /// Zero ticks at `rate`.
    #[must_use]
    pub const fn zero(rate: Rate) -> Self {
        Self::new(0, rate)
    }

    /// The tick count.
    #[must_use]
    pub const fn value(self) -> i64 {
        self.value
    }

    /// The rate the ticks are counted at.
    #[must_use]
    pub const fn rate(self) -> Rate {
        self.rate
    }

    /// True when this is zero ticks, whatever the rate.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.value == 0
    }

    /// The sum, expressed at this time's rate.
    ///
    /// # Errors
    ///
    /// Returns [`codes::INVALID_TIME`] when `other` is at a different rate that
    /// does not divide into this one exactly, or when the sum overflows.
    pub fn checked_add(self, other: Self) -> Result<Self> {
        let other = other.rescaled_to(self.rate)?;
        let value = self.value.checked_add(other.value).ok_or_else(|| {
            Error::new(codes::INVALID_TIME, "time sum overflows")
                .with("left", self.value)
                .with("right", other.value)
        })?;
        Ok(Self::new(value, self.rate))
    }

    /// The same instant counted at `rate`.
    ///
    /// # Errors
    ///
    /// Returns [`codes::INVALID_TIME`] when the instant is not exactly
    /// representable at `rate`: rescaling is only allowed when it loses
    /// nothing, so 1 tick at 24 fps becomes 2 at 48 fps but 1 tick at 25 fps
    /// is not expressible at 24.
    pub fn rescaled_to(self, rate: Rate) -> Result<Self> {
        if self.rate == rate {
            return Ok(self);
        }
        // value * (rate / self.rate), kept in i128 so the intermediate product
        // of two u32-scaled i64s cannot overflow.
        let numerator = i128::from(self.value)
            * i128::from(rate.numerator())
            * i128::from(self.rate.denominator());
        let denominator = i128::from(self.rate.numerator()) * i128::from(rate.denominator());
        if numerator % denominator != 0 {
            return Err(Error::new(
                codes::INVALID_TIME,
                "time is not exactly representable at the target rate",
            )
            .with("value", self.value)
            .with(
                "from_rate",
                format!("{}/{}", self.rate.numerator(), self.rate.denominator()),
            )
            .with(
                "to_rate",
                format!("{}/{}", rate.numerator(), rate.denominator()),
            ));
        }
        let value = i64::try_from(numerator / denominator).map_err(|_| {
            Error::new(codes::INVALID_TIME, "rescaled time overflows").with("value", self.value)
        })?;
        Ok(Self::new(value, rate))
    }

    /// The value as OTIO writes it. Only the serialiser calls this.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "OTIO's own encoding is a double; a tick count large enough to lose precision is \
                  older than the universe at any sane rate"
    )]
    pub fn as_f64(self) -> f64 {
        self.value as f64
    }

    /// Reads an OTIO tick count, which must be a whole number.
    ///
    /// # Errors
    ///
    /// Returns [`codes::INVALID_TIME`] when the number is not finite, not a
    /// whole number, or outside `i64`.
    pub fn from_f64(value: f64, rate: Rate) -> Result<Self> {
        if !value.is_finite() || value.fract() != 0.0 {
            return Err(Error::new(
                codes::INVALID_TIME,
                "time value must be a whole number of ticks",
            )
            .with("value", value));
        }
        if value.abs() > EXACT_WHOLE_LIMIT {
            return Err(Error::new(
                codes::INVALID_TIME,
                "time value is outside the range an f64 counts exactly",
            )
            .with("value", value));
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the magnitude check above keeps the value well within i64"
        )]
        let ticks = value as i64;
        Ok(Self::new(ticks, rate))
    }
}

/// A half-open span: `[start, start + duration)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    start: Time,
    duration: Time,
}

impl Span {
    /// Creates a span.
    ///
    /// # Errors
    ///
    /// Returns [`codes::INVALID_TIME`] when the duration is negative.
    pub fn new(start: Time, duration: Time) -> Result<Self> {
        if duration.value() < 0 {
            return Err(Error::new(
                codes::INVALID_TIME,
                "a time range may not last a negative time",
            )
            .with("duration", duration.value()));
        }
        Ok(Self { start, duration })
    }

    /// A zero-length span at `start`.
    #[must_use]
    pub fn point(start: Time) -> Self {
        Self {
            start,
            duration: Time::zero(start.rate()),
        }
    }

    /// The first instant in the span.
    #[must_use]
    pub const fn start(self) -> Time {
        self.start
    }

    /// The length of the span.
    #[must_use]
    pub const fn duration(self) -> Time {
        self.duration
    }
}

/// The greatest common divisor, for reducing a rate.
const fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

/// True when `value` is within [`RATE_TOLERANCE`] of `candidate`, relatively.
fn close_enough(value: f64, candidate: f64) -> bool {
    (value - candidate).abs() <= RATE_TOLERANCE * value.abs().max(1.0)
}

/// The simplest fraction reproducing `value`, by continued-fraction expansion.
///
/// Returns `None` when no fraction with a denominator up to
/// [`MAX_INVENTED_DENOMINATOR`] is close enough.
fn simplest_fraction(value: f64) -> Option<Rate> {
    // The convergents of the continued fraction: `numerator`/`denominator` is
    // the latest, and the pair before it is what the recurrence needs.
    let (mut previous_numerator, mut numerator) = (0_i64, 1_i64);
    let (mut previous_denominator, mut denominator) = (1_i64, 0_i64);
    let mut remainder = value;

    for _ in 0..40 {
        let whole = remainder.floor();
        if !whole.is_finite() || whole.abs() > f64::from(u32::MAX) {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the magnitude check above keeps this within i64"
        )]
        let whole_i = whole as i64;
        let next_numerator = whole_i
            .checked_mul(numerator)?
            .checked_add(previous_numerator)?;
        let next_denominator = whole_i
            .checked_mul(denominator)?
            .checked_add(previous_denominator)?;
        previous_numerator = numerator;
        previous_denominator = denominator;
        numerator = next_numerator;
        denominator = next_denominator;

        if denominator > i64::from(MAX_INVENTED_DENOMINATOR) || numerator > i64::from(u32::MAX) {
            return None;
        }
        if numerator > 0 && denominator > 0 {
            let candidate = Rate::new(
                u32::try_from(numerator).ok()?,
                u32::try_from(denominator).ok()?,
            )?;
            if close_enough(value, candidate.as_f64()) {
                return Some(candidate);
            }
        }

        let fraction = remainder - whole;
        if fraction <= 0.0 {
            return None;
        }
        remainder = 1.0 / fraction;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_is_stored_reduced_and_rejects_zero() {
        let rate = Rate::new(48, 2).unwrap();
        assert_eq!((rate.numerator(), rate.denominator()), (24, 1));
        assert_eq!(Rate::new(0, 1), None);
        assert_eq!(Rate::new(24, 0), None);
    }

    #[test]
    fn ntsc_rates_survive_a_round_trip_through_the_otio_double() {
        for (numerator, denominator) in [
            (24_000_u32, 1001_u32),
            (30_000, 1001),
            (60_000, 1001),
            (120_000, 1001),
        ] {
            let rate = Rate::new(numerator, denominator).unwrap();
            assert_eq!(Rate::from_f64(rate.as_f64()).unwrap(), rate);
        }
    }

    #[test]
    fn whole_and_fractional_rates_round_trip_too() {
        for (numerator, denominator) in [(24_u32, 1_u32), (25, 1), (48_000, 1), (50, 3), (100, 7)] {
            let rate = Rate::new(numerator, denominator).unwrap();
            assert_eq!(Rate::from_f64(rate.as_f64()).unwrap(), rate);
        }
    }

    #[test]
    fn an_unusable_rate_is_reported_rather_than_approximated() {
        for bad in [f64::NAN, f64::INFINITY, 0.0, -24.0, 1e-9] {
            let error = Rate::from_f64(bad).unwrap_err();
            assert_eq!(error.code, codes::INVALID_TIME);
        }
    }

    #[test]
    fn rescaling_is_exact_or_it_is_an_error() {
        let rate_24 = Rate::FPS_24;
        let rate_48 = Rate::new(48, 1).unwrap();
        let one_at_24 = Time::new(1, rate_24);
        assert_eq!(
            one_at_24.rescaled_to(rate_48).unwrap(),
            Time::new(2, rate_48)
        );
        assert_eq!(one_at_24.rescaled_to(rate_24).unwrap(), one_at_24);

        let one_at_25 = Time::new(1, Rate::new(25, 1).unwrap());
        assert_eq!(
            one_at_25.rescaled_to(rate_24).unwrap_err().code,
            codes::INVALID_TIME
        );
    }

    #[test]
    fn adding_rescales_the_right_hand_side() {
        let rate_24 = Rate::FPS_24;
        let rate_48 = Rate::new(48, 1).unwrap();
        let sum = Time::new(10, rate_24)
            .checked_add(Time::new(4, rate_48))
            .unwrap();
        assert_eq!(sum, Time::new(12, rate_24));
    }

    #[test]
    fn a_fractional_tick_count_is_refused() {
        assert_eq!(
            Time::from_f64(1.5, Rate::FPS_24).unwrap_err().code,
            codes::INVALID_TIME
        );
        assert_eq!(Time::from_f64(-12.0, Rate::FPS_24).unwrap().value(), -12);
    }

    #[test]
    fn a_span_may_not_last_a_negative_time() {
        let rate = Rate::FPS_24;
        assert_eq!(
            Span::new(Time::zero(rate), Time::new(-1, rate))
                .unwrap_err()
                .code,
            codes::INVALID_TIME
        );
        assert!(Span::point(Time::new(5, rate)).duration().is_zero());
    }
}
