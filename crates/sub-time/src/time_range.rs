//! Half-open ranges of [`RationalTime`].

use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rational_time::RationalTime;

/// A half-open range `[start, start + duration)` with a non-negative duration.
///
/// Ranges are half-open so that adjacent clips neither overlap nor leave a gap:
/// a range ending at `t` and one starting at `t` are butt-joined. `start` and
/// `duration` may be expressed at different rates; comparisons are exact
/// regardless (see [`RationalTime`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(into = "TimeRangeRepr", try_from = "TimeRangeRepr")]
#[schemars(with = "TimeRangeRepr")]
pub struct TimeRange {
    start: RationalTime,
    duration: RationalTime,
}

/// The serde form of a [`TimeRange`].
///
/// Reading goes through [`TimeRange::new`], so a file can never yield a range
/// with a negative duration or an end that is not representable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TimeRangeRepr {
    /// The inclusive start.
    start: RationalTime,
    /// The length; never negative.
    duration: RationalTime,
}

impl From<TimeRange> for TimeRangeRepr {
    fn from(range: TimeRange) -> Self {
        Self {
            start: range.start,
            duration: range.duration,
        }
    }
}

impl TryFrom<TimeRangeRepr> for TimeRange {
    type Error = InvalidTimeRange;

    fn try_from(repr: TimeRangeRepr) -> Result<Self, Self::Error> {
        Self::new(repr.start, repr.duration).ok_or(InvalidTimeRange)
    }
}

/// A start and duration that do not form a valid [`TimeRange`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTimeRange;

impl fmt::Display for InvalidTimeRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("time range duration must be non-negative and its end representable")
    }
}

impl core::error::Error for InvalidTimeRange {}

impl TimeRange {
    /// Creates a range starting at `start` and lasting `duration`.
    ///
    /// Returns `None` if `duration` is negative, or if `start + duration`
    /// cannot be represented exactly.
    pub fn new(start: RationalTime, duration: RationalTime) -> Option<Self> {
        if duration.is_negative() {
            return None;
        }
        // Reject up front what `end_exclusive` could not compute later.
        start.checked_add(duration)?;
        Some(Self { start, duration })
    }

    /// Creates an empty range at `start`.
    pub fn empty_at(start: RationalTime) -> Self {
        Self {
            start,
            duration: RationalTime::zero(start.rate()),
        }
    }

    /// Creates a range from its inclusive start and exclusive end.
    ///
    /// Returns `None` if `end` precedes `start` or the duration is not exactly
    /// representable.
    pub fn from_start_end(start: RationalTime, end_exclusive: RationalTime) -> Option<Self> {
        let duration = end_exclusive.checked_sub(start)?;
        Self::new(start, duration)
    }

    /// The inclusive start of the range.
    pub const fn start(self) -> RationalTime {
        self.start
    }

    /// The length of the range; never negative.
    pub const fn duration(self) -> RationalTime {
        self.duration
    }

    /// The exclusive end of the range, `start + duration`.
    ///
    /// # Panics
    ///
    /// Cannot panic: every constructor rejects a range whose end is not
    /// representable.
    pub fn end_exclusive(self) -> RationalTime {
        self.start
            .checked_add(self.duration)
            .expect("range end is representable by construction")
    }

    /// True if the range has zero length.
    pub fn is_empty(self) -> bool {
        self.duration.is_zero()
    }

    /// True if `time` falls in `[start, end)`. An empty range contains nothing.
    pub fn contains(self, time: RationalTime) -> bool {
        time >= self.start && time < self.end_exclusive()
    }

    /// True if every instant of `other` is contained in `self`.
    ///
    /// An empty `other` is contained when its start lies in `self`.
    pub fn contains_range(self, other: Self) -> bool {
        if other.is_empty() {
            return self.contains(other.start);
        }
        other.start >= self.start && other.end_exclusive() <= self.end_exclusive()
    }

    /// True if the two ranges share at least one instant.
    ///
    /// Butt-joined ranges (one ending where the next starts) do not overlap,
    /// and an empty range overlaps nothing.
    pub fn overlaps(self, other: Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.start < other.end_exclusive()
            && other.start < self.end_exclusive()
    }

    /// Clamps `time` into `[start, end_exclusive]`.
    ///
    /// The upper bound is the exclusive end itself, which is the useful answer
    /// for a playhead or a trim handle: a time past the range clamps to the
    /// instant the range ends at, and an empty range clamps everything to its
    /// start. Use [`Self::contains`] when you need the half-open test.
    pub fn clamp(self, time: RationalTime) -> RationalTime {
        if time < self.start {
            return self.start;
        }
        let end = self.end_exclusive();
        if time > end { end } else { time }
    }

    /// The overlapping part of the two ranges, or `None` when they do not
    /// overlap.
    pub fn intersection(self, other: Self) -> Option<Self> {
        if !self.overlaps(other) {
            return None;
        }
        let start = self.start.max(other.start);
        let end = self.end_exclusive().min(other.end_exclusive());
        Self::from_start_end(start, end)
    }

    /// Moves the range by `offset`, keeping its duration.
    ///
    /// Returns `None` if the shifted start is not exactly representable.
    pub fn shifted_by(self, offset: RationalTime) -> Option<Self> {
        Self::new(self.start.checked_add(offset)?, self.duration)
    }
}

impl fmt::Display for TimeRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{} +{})", self.start, self.duration)
    }
}

#[cfg(test)]
mod tests {
    use super::TimeRange;
    use crate::rational::Rational;
    use crate::rational_time::RationalTime;

    const NTSC_24: Rational = Rational::FPS_23_976;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, Rational::FPS_24)
    }

    fn range(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(frames(start), frames(duration)).unwrap()
    }

    #[test]
    fn rejects_negative_duration() {
        assert!(TimeRange::new(frames(0), frames(-1)).is_none());
        assert!(TimeRange::from_start_end(frames(10), frames(5)).is_none());
    }

    #[test]
    fn start_duration_and_end() {
        let r = range(10, 5);
        assert_eq!(r.start(), frames(10));
        assert_eq!(r.duration(), frames(5));
        assert_eq!(r.end_exclusive(), frames(15));
        assert!(!r.is_empty());
        assert!(TimeRange::empty_at(frames(3)).is_empty());
        assert_eq!(r.to_string(), "[10@24 +5@24)");
    }

    #[test]
    fn contains_is_half_open() {
        let r = range(10, 5);
        assert!(r.contains(frames(10)));
        assert!(r.contains(frames(14)));
        assert!(!r.contains(frames(15)));
        assert!(!r.contains(frames(9)));
        assert!(!TimeRange::empty_at(frames(10)).contains(frames(10)));
    }

    #[test]
    fn contains_across_rates() {
        let r =
            TimeRange::new(RationalTime::from_seconds(1), RationalTime::from_seconds(1)).unwrap();
        assert!(r.contains(RationalTime::new(24, Rational::FPS_24)));
        assert!(r.contains(RationalTime::new(47, Rational::FPS_24)));
        assert!(!r.contains(RationalTime::new(48, Rational::FPS_24)));
        // 24 frames of 23.976 fps material is 1.001 s: inside, unlike 48 of
        // them (2.002 s), which lands past the exclusive end.
        assert!(r.contains(RationalTime::new(24, NTSC_24)));
        assert!(!r.contains(RationalTime::new(48, NTSC_24)));
        assert!(r.contains_range(range(24, 24)));
        assert!(!r.contains_range(range(24, 25)));
    }

    #[test]
    fn overlaps_excludes_butt_joins_and_empties() {
        assert!(range(10, 5).overlaps(range(14, 5)));
        assert!(range(14, 5).overlaps(range(10, 5)));
        assert!(
            !range(10, 5).overlaps(range(15, 5)),
            "butt-joined clips do not overlap"
        );
        assert!(!range(10, 5).overlaps(range(20, 5)));
        assert!(!range(10, 5).overlaps(TimeRange::empty_at(frames(12))));
    }

    #[test]
    fn intersection_of_overlapping_ranges() {
        assert_eq!(
            range(10, 5).intersection(range(12, 10)).unwrap(),
            range(12, 3)
        );
        assert_eq!(
            range(10, 5).intersection(range(0, 100)).unwrap(),
            range(10, 5)
        );
        assert!(range(10, 5).intersection(range(15, 5)).is_none());
        assert!(
            range(10, 5)
                .intersection(TimeRange::empty_at(frames(11)))
                .is_none()
        );
    }

    #[test]
    fn intersection_across_ntsc_and_integral_rates() {
        // 24000 frames at 24000/1001 fps is exactly 1001 seconds.
        let ntsc = TimeRange::new(
            RationalTime::zero(NTSC_24),
            RationalTime::new(24000, NTSC_24),
        )
        .unwrap();
        let secs = TimeRange::new(
            RationalTime::from_seconds(1000),
            RationalTime::from_seconds(10),
        )
        .unwrap();
        let hit = ntsc.intersection(secs).unwrap();
        assert_eq!(hit.start(), RationalTime::from_seconds(1000));
        assert_eq!(hit.end_exclusive(), RationalTime::from_seconds(1001));
        assert_eq!(hit.duration(), RationalTime::from_seconds(1));
    }

    #[test]
    fn clamp_pins_to_the_bounds() {
        let r = range(10, 5);
        assert_eq!(r.clamp(frames(0)), frames(10));
        assert_eq!(r.clamp(frames(12)), frames(12));
        assert_eq!(r.clamp(frames(99)), frames(15));
        let empty = TimeRange::empty_at(frames(7));
        assert_eq!(empty.clamp(frames(0)), frames(7));
        assert_eq!(empty.clamp(frames(9)), frames(7));
    }

    #[test]
    fn shifted_by_keeps_duration() {
        let r = range(10, 5).shifted_by(frames(-4)).unwrap();
        assert_eq!(r, range(6, 5));
    }

    #[test]
    fn equality_ignores_representation() {
        let a = range(24, 24);
        let b =
            TimeRange::new(RationalTime::from_seconds(1), RationalTime::from_seconds(1)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn serde_round_trips_and_rejects_a_negative_duration() {
        let range = TimeRange::new(frames(12), frames(48)).unwrap();
        let json = serde_json::to_string(&range).unwrap();
        assert_eq!(serde_json::from_str::<TimeRange>(&json).unwrap(), range);

        let negative = serde_json::to_string(&super::TimeRangeRepr {
            start: frames(12),
            duration: frames(-1),
        })
        .unwrap();
        let err = serde_json::from_str::<TimeRange>(&negative).unwrap_err();
        assert!(err.to_string().contains("non-negative"), "{err}");
    }
}
