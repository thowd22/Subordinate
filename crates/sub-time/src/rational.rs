//! Exact rational rates (frames or samples per second).

use core::fmt;

/// Greatest common divisor of two non-negative integers.
pub(crate) const fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// A strictly positive rational rate in units per second, held in lowest terms.
///
/// A rate of `numerator / denominator` means that one unit of a
/// [`RationalTime`](crate::RationalTime) at this rate lasts
/// `denominator / numerator` seconds. NTSC rates are represented exactly:
/// 23.976 fps is `24000/1001`, never a float.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rational {
    numerator: u32,
    denominator: u32,
}

impl Rational {
    /// One unit per second, the natural rate for whole seconds.
    pub const ONE: Self = Self {
        numerator: 1,
        denominator: 1,
    };
    /// 23.976 fps, exactly `24000/1001`.
    pub const FPS_23_976: Self = Self {
        numerator: 24000,
        denominator: 1001,
    };
    /// 24 fps.
    pub const FPS_24: Self = Self {
        numerator: 24,
        denominator: 1,
    };
    /// 25 fps.
    pub const FPS_25: Self = Self {
        numerator: 25,
        denominator: 1,
    };
    /// 29.97 fps, exactly `30000/1001`.
    pub const FPS_29_97: Self = Self {
        numerator: 30000,
        denominator: 1001,
    };
    /// 30 fps.
    pub const FPS_30: Self = Self {
        numerator: 30,
        denominator: 1,
    };
    /// 50 fps.
    pub const FPS_50: Self = Self {
        numerator: 50,
        denominator: 1,
    };
    /// 59.94 fps, exactly `60000/1001`.
    pub const FPS_59_94: Self = Self {
        numerator: 60000,
        denominator: 1001,
    };
    /// 60 fps.
    pub const FPS_60: Self = Self {
        numerator: 60,
        denominator: 1,
    };
    /// 48 kHz audio sample rate.
    pub const HZ_48000: Self = Self {
        numerator: 48000,
        denominator: 1,
    };

    /// Creates a rate of `numerator / denominator` units per second, reduced to
    /// lowest terms.
    ///
    /// Returns `None` if either part is zero, since a rate must be strictly
    /// positive.
    #[allow(clippy::cast_possible_truncation)]
    pub const fn new(numerator: u32, denominator: u32) -> Option<Self> {
        if numerator == 0 || denominator == 0 {
            return None;
        }
        // The gcd of two `u32` values always fits in a `u32`.
        let g = gcd_u128(numerator as u128, denominator as u128) as u32;
        Some(Self {
            numerator: numerator / g,
            denominator: denominator / g,
        })
    }

    /// Creates an integer rate of `units` per second.
    ///
    /// Returns `None` if `units` is zero.
    pub const fn from_integer(units: u32) -> Option<Self> {
        Self::new(units, 1)
    }

    /// The numerator of the reduced rate.
    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    /// The denominator of the reduced rate.
    pub const fn denominator(self) -> u32 {
        self.denominator
    }

    /// True if the rate is a whole number of units per second.
    pub const fn is_integral(self) -> bool {
        self.denominator == 1
    }

    /// The smallest rate at which values at both `self` and `other` are exactly
    /// representable: an integer multiple of each of the two rates.
    ///
    /// Returns `None` if that rate would not fit in a [`Rational`].
    pub fn common_rate(self, other: Self) -> Option<Self> {
        // A rate is n/d, so the common rate is lcm(n_a, n_b) / gcd(d_a, d_b);
        // dividing it by either input rate yields an integer.
        let na = u128::from(self.numerator);
        let nb = u128::from(other.numerator);
        let lcm = na / gcd_u128(na, nb) * nb;
        let den = gcd_u128(u128::from(self.denominator), u128::from(other.denominator));
        let num = u32::try_from(lcm).ok()?;
        let den = u32::try_from(den).ok()?;
        Self::new(num, den)
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.denominator == 1 {
            write!(f, "{}", self.numerator)
        } else {
            write!(f, "{}/{}", self.numerator, self.denominator)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rational;

    #[test]
    fn rejects_zero() {
        assert!(Rational::new(0, 1).is_none());
        assert!(Rational::new(24, 0).is_none());
    }

    #[test]
    fn reduces_to_lowest_terms() {
        let r = Rational::new(48, 2).unwrap();
        assert_eq!(r, Rational::FPS_24);
        assert_eq!(r.numerator(), 24);
        assert_eq!(r.denominator(), 1);
    }

    #[test]
    fn ntsc_rates_stay_exact() {
        assert_eq!(Rational::FPS_23_976.numerator(), 24000);
        assert_eq!(Rational::FPS_23_976.denominator(), 1001);
        assert!(!Rational::FPS_23_976.is_integral());
        assert_eq!(Rational::FPS_23_976.to_string(), "24000/1001");
        assert_eq!(Rational::FPS_24.to_string(), "24");
    }

    #[test]
    fn common_rate_of_ntsc_pair() {
        let common = Rational::FPS_23_976
            .common_rate(Rational::FPS_29_97)
            .unwrap();
        assert_eq!(common, Rational::new(120_000, 1001).unwrap());
    }

    #[test]
    fn common_rate_of_integral_pair() {
        let common = Rational::FPS_24.common_rate(Rational::FPS_25).unwrap();
        assert_eq!(common, Rational::from_integer(600).unwrap());
        assert_eq!(
            Rational::FPS_30.common_rate(Rational::FPS_60).unwrap(),
            Rational::FPS_60
        );
    }

    #[test]
    fn common_rate_overflow_is_none() {
        let a = Rational::from_integer(4_000_000_007).unwrap();
        let b = Rational::from_integer(4_000_000_009).unwrap();
        assert!(a.common_rate(b).is_none());
    }
}
