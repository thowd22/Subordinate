//! Per-clip parameter values: fixed-point scalars, opacity, gain and transform.
//!
//! The MVP supports a fixed set of clip parameters (docs/PLAN.md §2 and §5.1):
//! opacity, a position/scale/rotation transform, an audio gain in decibels and
//! fade in/out durations. Modelling them explicitly keeps the compositor and
//! the mixer simple.
//!
//! Every scalar here is a [`Fixed6`], an exact six-decimal fixed-point number,
//! never a float. That keeps clips comparable and hashable, keeps saved
//! projects byte-identical across platforms, and makes a NaN unrepresentable.
//! The compositor and the mixer convert once, at the edge, with
//! [`Fixed6::as_f32`].

use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};

use crate::codes;

/// Micro-units in one whole unit.
const MICROS_PER_UNIT: i64 = 1_000_000;
/// [`MICROS_PER_UNIT`] as `u64`, for the unsigned formatting path.
const MICROS_PER_UNIT_U64: u64 = 1_000_000;
/// [`MICROS_PER_UNIT`] as `f64`, for float conversion.
const MICROS_PER_UNIT_F64: f64 = 1.0e6;
/// The largest magnitude, in whole units, that a float may carry into a
/// [`Fixed6`]. Well inside both `i64` micro-units and the exactly representable
/// `f64` integers.
const MAX_UNITS: f64 = 9.0e12;

/// An exact number with six decimal places, stored as a count of micro-units.
///
/// This is the one numeric primitive for the non-time clip parameters. It is
/// `Ord` and `Hash`, so clips compare and hash exactly, and it saves as an
/// integer.
///
/// The serde form is the raw micro-unit count: an integer, never a float.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct Fixed6(i64);

impl Fixed6 {
    /// Zero.
    pub const ZERO: Self = Self(0);
    /// One whole unit.
    pub const ONE: Self = Self(MICROS_PER_UNIT);

    /// Creates a value from a raw count of micro-units.
    #[must_use]
    pub const fn from_micros(micros: i64) -> Self {
        Self(micros)
    }

    /// Creates a value from a whole number of units.
    #[must_use]
    pub fn from_units(units: i32) -> Self {
        Self(i64::from(units) * MICROS_PER_UNIT)
    }

    /// Creates a value from a float, rounding to the nearest micro-unit.
    ///
    /// This is the entry point for a UI slider or a plugin parameter; the
    /// model never carries the float onwards.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_parameter` if `value` is NaN, infinite, or of a
    /// magnitude beyond what six-decimal fixed point holds.
    // The range check above bounds the product well inside `i64`.
    #[allow(clippy::cast_possible_truncation)]
    pub fn from_f64(value: f64) -> SubResult<Self> {
        if !value.is_finite() || value.abs() > MAX_UNITS {
            return Err(SubError::new(
                codes::INVALID_PARAMETER,
                "parameter value must be a finite number within the fixed-point range",
            )
            .with_detail("value", value)
            .with_detail("max_magnitude", MAX_UNITS));
        }
        Ok(Self((value * MICROS_PER_UNIT_F64).round() as i64))
    }

    /// The raw count of micro-units.
    #[must_use]
    pub const fn micros(self) -> i64 {
        self.0
    }

    /// True when the value is below zero.
    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    /// True when the value is exactly zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// The value as `f64`. Micro-unit counts in range are exactly
    /// representable.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn as_f64(self) -> f64 {
        self.0 as f64 / MICROS_PER_UNIT_F64
    }

    /// The value as `f32`, for the compositor and the mixer.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn as_f32(self) -> f32 {
        self.as_f64() as f32
    }
}

impl fmt::Display for Fixed6 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let magnitude = self.0.unsigned_abs();
        let whole = magnitude / MICROS_PER_UNIT_U64;
        let fraction = magnitude % MICROS_PER_UNIT_U64;
        if fraction == 0 {
            return write!(f, "{sign}{whole}");
        }
        let mut digits = format!("{fraction:06}");
        while digits.ends_with('0') {
            digits.pop();
        }
        write!(f, "{sign}{whole}.{digits}")
    }
}

/// How opaque a clip is, from fully transparent to fully opaque.
///
/// The compositor multiplies the sampled clip by this factor before blending
/// (docs/PLAN.md §5.2).
///
/// The serde form is the factor as a [`Fixed6`], validated on the way in.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(into = "Fixed6", try_from = "Fixed6")]
#[schemars(
    with = "Fixed6",
    description = "Opacity in micro-units, from 0 to 1000000."
)]
pub struct Opacity(Fixed6);

impl From<Opacity> for Fixed6 {
    fn from(opacity: Opacity) -> Self {
        opacity.0
    }
}

impl TryFrom<Fixed6> for Opacity {
    type Error = SubError;

    fn try_from(factor: Fixed6) -> SubResult<Self> {
        Self::new(factor)
    }
}

impl Opacity {
    /// Fully opaque, the default.
    pub const OPAQUE: Self = Self(Fixed6::ONE);
    /// Fully transparent.
    pub const TRANSPARENT: Self = Self(Fixed6::ZERO);

    /// Creates an opacity from a factor in `0..=1`.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_parameter` if the factor is outside `0..=1`.
    pub fn new(factor: Fixed6) -> SubResult<Self> {
        if factor < Fixed6::ZERO || factor > Fixed6::ONE {
            return Err(SubError::new(
                codes::INVALID_PARAMETER,
                "opacity must be between 0 and 1 inclusive",
            )
            .with_detail("factor", factor.to_string()));
        }
        Ok(Self(factor))
    }

    /// Creates an opacity from a float factor in `0.0..=1.0`.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_parameter` if the factor is not a finite number
    /// in `0.0..=1.0`.
    pub fn from_f64(factor: f64) -> SubResult<Self> {
        Self::new(Fixed6::from_f64(factor)?)
    }

    /// The factor, exactly.
    #[must_use]
    pub const fn factor(self) -> Fixed6 {
        self.0
    }

    /// The factor as `f32`, for the compositor.
    #[must_use]
    pub fn as_f32(self) -> f32 {
        self.0.as_f32()
    }

    /// True when the clip contributes nothing to the composite.
    #[must_use]
    pub const fn is_transparent(self) -> bool {
        self.0.is_zero()
    }
}

impl Default for Opacity {
    /// Fully opaque.
    fn default() -> Self {
        Self::OPAQUE
    }
}

/// An audio level in decibels relative to unity.
///
/// Decibels, not a linear factor, because that is what the mixer shows and
/// what a fader interpolates in (docs/PLAN.md §5.4).
///
/// The serde form is the level as a [`Fixed6`], validated on the way in.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(into = "Fixed6", try_from = "Fixed6")]
#[schemars(
    with = "Fixed6",
    description = "Gain in micro-decibels, from -144000000 to 24000000."
)]
pub struct GainDb(Fixed6);

impl From<GainDb> for Fixed6 {
    fn from(gain: GainDb) -> Self {
        gain.0
    }
}

impl TryFrom<Fixed6> for GainDb {
    type Error = SubError;

    fn try_from(decibels: Fixed6) -> SubResult<Self> {
        Self::new(decibels)
    }
}

impl GainDb {
    /// The lowest accepted level, -144 dB, treated as silence.
    pub const MIN: Fixed6 = Fixed6::from_micros(-144 * MICROS_PER_UNIT);
    /// The highest accepted level, +24 dB.
    pub const MAX: Fixed6 = Fixed6::from_micros(24 * MICROS_PER_UNIT);
    /// Unity gain (0 dB), the default: the clip passes through untouched.
    pub const UNITY: Self = Self(Fixed6::ZERO);
    /// The bottom of the fader.
    pub const SILENT: Self = Self(Self::MIN);

    /// Creates a gain from a level in decibels.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_parameter` if the level is outside
    /// [`GainDb::MIN`]`..=`[`GainDb::MAX`].
    pub fn new(decibels: Fixed6) -> SubResult<Self> {
        if decibels < Self::MIN || decibels > Self::MAX {
            return Err(SubError::new(
                codes::INVALID_PARAMETER,
                "gain must be between -144 dB and +24 dB inclusive",
            )
            .with_detail("decibels", decibels.to_string()));
        }
        Ok(Self(decibels))
    }

    /// Creates a gain from a float level in decibels.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_parameter` if the level is not a finite number
    /// in the accepted range.
    pub fn from_f64(decibels: f64) -> SubResult<Self> {
        Self::new(Fixed6::from_f64(decibels)?)
    }

    /// The level in decibels, exactly.
    #[must_use]
    pub const fn decibels(self) -> Fixed6 {
        self.0
    }

    /// The level in decibels as `f32`, for the mixer.
    #[must_use]
    pub fn as_f32(self) -> f32 {
        self.0.as_f32()
    }

    /// True when the gain is at the bottom of the fader.
    #[must_use]
    pub fn is_silent(self) -> bool {
        self.0 <= Self::MIN
    }
}

impl Default for GainDb {
    /// Unity gain.
    fn default() -> Self {
        Self::UNITY
    }
}

/// A point on the sequence canvas, in pixels, with sub-pixel precision.
///
/// The origin is the centre of the canvas: positive `x` moves right, positive
/// `y` moves down.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct Point2 {
    /// Horizontal offset in pixels.
    pub x: Fixed6,
    /// Vertical offset in pixels.
    pub y: Fixed6,
}

impl Point2 {
    /// The centre of the canvas, `(0, 0)`.
    pub const ORIGIN: Self = Self {
        x: Fixed6::ZERO,
        y: Fixed6::ZERO,
    };

    /// Creates a point.
    #[must_use]
    pub const fn new(x: Fixed6, y: Fixed6) -> Self {
        Self { x, y }
    }
}

/// A per-axis scale factor. Neither axis may be zero; a negative axis mirrors.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(into = "Scale2Repr", try_from = "Scale2Repr")]
#[schemars(with = "Scale2Repr")]
pub struct Scale2 {
    x: Fixed6,
    y: Fixed6,
}

/// The serde form of a [`Scale2`], validated on the way in by
/// [`Scale2::new`] so a file can never collapse a clip to nothing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Scale2Repr {
    /// The horizontal factor; never zero.
    x: Fixed6,
    /// The vertical factor; never zero.
    y: Fixed6,
}

impl From<Scale2> for Scale2Repr {
    fn from(scale: Scale2) -> Self {
        Self {
            x: scale.x,
            y: scale.y,
        }
    }
}

impl TryFrom<Scale2Repr> for Scale2 {
    type Error = SubError;

    fn try_from(repr: Scale2Repr) -> SubResult<Self> {
        Self::new(repr.x, repr.y)
    }
}

impl Scale2 {
    /// No scaling, `(1, 1)`.
    pub const UNIFORM: Self = Self {
        x: Fixed6::ONE,
        y: Fixed6::ONE,
    };

    /// Creates a scale.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_parameter` if either axis is zero, which would
    /// collapse the clip to nothing and make the transform non-invertible.
    pub fn new(x: Fixed6, y: Fixed6) -> SubResult<Self> {
        if x.is_zero() || y.is_zero() {
            return Err(SubError::new(
                codes::INVALID_PARAMETER,
                "scale must be non-zero on both axes",
            )
            .with_detail("x", x.to_string())
            .with_detail("y", y.to_string()));
        }
        Ok(Self { x, y })
    }

    /// Creates a scale that is the same on both axes.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_parameter` if `factor` is zero.
    pub fn uniform(factor: Fixed6) -> SubResult<Self> {
        Self::new(factor, factor)
    }

    /// The horizontal factor.
    #[must_use]
    pub const fn x(self) -> Fixed6 {
        self.x
    }

    /// The vertical factor.
    #[must_use]
    pub const fn y(self) -> Fixed6 {
        self.y
    }
}

impl Default for Scale2 {
    /// No scaling.
    fn default() -> Self {
        Self::UNIFORM
    }
}

/// The 2D placement of a clip on the sequence canvas.
///
/// The compositor applies scale, then rotation, then translation, all about
/// the clip centre (docs/PLAN.md §5.2). OTIO has no counterpart; this would
/// live in `metadata` on an OTIO export.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    /// Offset of the clip centre from the canvas centre, in pixels.
    pub position: Point2,
    /// Per-axis scale factor.
    pub scale: Scale2,
    /// Clockwise rotation about the clip centre, in degrees.
    pub rotation_degrees: Fixed6,
}

impl Transform {
    /// Centred, unscaled and unrotated: the default.
    pub const IDENTITY: Self = Self {
        position: Point2::ORIGIN,
        scale: Scale2::UNIFORM,
        rotation_degrees: Fixed6::ZERO,
    };

    /// Creates a transform. Each component is validated by its own type, so
    /// this cannot fail.
    #[must_use]
    pub const fn new(position: Point2, scale: Scale2, rotation_degrees: Fixed6) -> Self {
        Self {
            position,
            scale,
            rotation_degrees,
        }
    }

    /// True when the transform leaves the clip untouched.
    #[must_use]
    pub fn is_identity(self) -> bool {
        self == Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_round_trips_through_micro_units() {
        assert_eq!(Fixed6::ONE.micros(), 1_000_000);
        assert_eq!(Fixed6::from_units(-3).micros(), -3_000_000);
        assert!(Fixed6::from_units(-3).is_negative());
        assert!(Fixed6::ZERO.is_zero());
        assert!((Fixed6::from_micros(1_500_000).as_f64() - 1.5).abs() < f64::EPSILON);
        assert!((Fixed6::from_micros(-250_000).as_f32() + 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn fixed_point_from_float_rounds_to_the_nearest_micro_unit() {
        assert_eq!(Fixed6::from_f64(0.5).unwrap().micros(), 500_000);
        assert_eq!(Fixed6::from_f64(0.000_000_4).unwrap(), Fixed6::ZERO);
        assert_eq!(Fixed6::from_f64(-0.000_000_6).unwrap().micros(), -1);
    }

    #[test]
    fn fixed_point_rejects_non_finite_and_oversized_floats() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.0e13, -1.0e13] {
            let err = Fixed6::from_f64(bad).unwrap_err();
            assert_eq!(err.code, codes::INVALID_PARAMETER);
        }
    }

    #[test]
    fn fixed_point_displays_without_trailing_zeros() {
        assert_eq!(Fixed6::ZERO.to_string(), "0");
        assert_eq!(Fixed6::from_units(7).to_string(), "7");
        assert_eq!(Fixed6::from_micros(-1_250_000).to_string(), "-1.25");
        assert_eq!(Fixed6::from_micros(1).to_string(), "0.000001");
    }

    #[test]
    fn opacity_accepts_the_unit_interval_and_nothing_else() {
        assert_eq!(Opacity::default(), Opacity::OPAQUE);
        assert!(Opacity::TRANSPARENT.is_transparent());
        assert_eq!(Opacity::from_f64(0.25).unwrap().factor().micros(), 250_000);
        assert!((Opacity::OPAQUE.as_f32() - 1.0).abs() < f32::EPSILON);

        for bad in [-0.000_001, 1.000_001, 2.0] {
            let err = Opacity::from_f64(bad).unwrap_err();
            assert_eq!(err.code, codes::INVALID_PARAMETER);
            assert!(err.details.contains_key("factor"));
        }
    }

    #[test]
    fn gain_accepts_the_fader_range_and_nothing_else() {
        assert_eq!(GainDb::default(), GainDb::UNITY);
        assert!(GainDb::SILENT.is_silent());
        assert!(!GainDb::UNITY.is_silent());
        assert_eq!(
            GainDb::from_f64(-6.0).unwrap().decibels().micros(),
            -6_000_000
        );
        assert!((GainDb::from_f64(3.5).unwrap().as_f32() - 3.5).abs() < f32::EPSILON);

        for bad in [-144.000_001, 24.000_001, -1000.0] {
            let err = GainDb::from_f64(bad).unwrap_err();
            assert_eq!(err.code, codes::INVALID_PARAMETER);
        }
    }

    #[test]
    fn scale_rejects_a_collapsed_axis_but_allows_a_mirror() {
        assert_eq!(Scale2::default(), Scale2::UNIFORM);
        let mirrored = Scale2::new(Fixed6::from_units(-1), Fixed6::ONE).unwrap();
        assert!(mirrored.x().is_negative());
        assert_eq!(mirrored.y(), Fixed6::ONE);
        assert_eq!(
            Scale2::uniform(Fixed6::from_units(2)).unwrap().y().micros(),
            2_000_000
        );

        for (x, y) in [(Fixed6::ZERO, Fixed6::ONE), (Fixed6::ONE, Fixed6::ZERO)] {
            let err = Scale2::new(x, y).unwrap_err();
            assert_eq!(err.code, codes::INVALID_PARAMETER);
        }
    }

    #[test]
    fn the_default_transform_is_the_identity() {
        assert_eq!(Transform::default(), Transform::IDENTITY);
        assert!(Transform::default().is_identity());
        assert_eq!(Point2::default(), Point2::ORIGIN);

        let moved = Transform::new(
            Point2::new(Fixed6::from_units(120), Fixed6::from_units(-40)),
            Scale2::UNIFORM,
            Fixed6::from_micros(90_500_000),
        );
        assert!(!moved.is_identity());
        assert_eq!(moved.position.x.to_string(), "120");
        assert_eq!(moved.position.y.to_string(), "-40");
        assert_eq!(moved.rotation_degrees.to_string(), "90.5");
    }
}
