//! The hand-off between the decode thread and the UI thread.
//!
//! A preview may drop frames (docs/PLAN.md §5.3), so the hand-off is a single
//! slot with a newest-wins policy rather than a queue: if the UI is a frame
//! behind, the older picture is worthless and holding on to it would only add
//! latency. The slot is what makes decode-to-display latency bounded by one
//! frame interval plus the upload, whatever the decoder is doing.

use std::sync::Mutex;
use std::time::Instant;

use sub_time::RationalTime;

use crate::nv12::Nv12Geometry;

/// One decoded picture on its way to the GPU.
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// Where its planes sit in `y` and `uv`.
    pub geometry: Nv12Geometry,
    /// Presentation timestamp, exact, in nanoseconds. Never a float.
    pub pts: RationalTime,
    /// Luma plane, strides included.
    pub y: Vec<u8>,
    /// Interleaved chroma plane, strides included.
    pub uv: Vec<u8>,
    /// When the appsink handed this frame over. The other end of the
    /// decode-to-display measurement.
    pub arrived: Instant,
}

/// The newest decoded frame, or nothing if the UI has already taken it.
#[derive(Debug, Default)]
pub struct FrameSlot {
    newest: Mutex<Option<DecodedFrame>>,
}

impl FrameSlot {
    /// An empty slot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `frame`, discarding whatever was waiting.
    ///
    /// Returns `true` when a frame was displaced, which is one dropped frame.
    pub fn put(&self, frame: DecodedFrame) -> bool {
        let mut newest = match self.newest.lock() {
            Ok(newest) => newest,
            Err(poisoned) => poisoned.into_inner(),
        };
        newest.replace(frame).is_some()
    }

    /// Take the waiting frame, if there is one.
    pub fn take(&self) -> Option<DecodedFrame> {
        let mut newest = match self.newest.lock() {
            Ok(newest) => newest,
            Err(poisoned) => poisoned.into_inner(),
        };
        newest.take()
    }

    /// Whether a frame is waiting.
    pub fn has_frame(&self) -> bool {
        match self.newest.lock() {
            Ok(newest) => newest.is_some(),
            Err(poisoned) => poisoned.into_inner().is_some(),
        }
    }
}

/// A running set of durations, kept as integer nanoseconds so the summary is
/// reproducible.
#[derive(Debug, Default, Clone)]
pub struct Durations {
    samples: Vec<u64>,
}

impl Durations {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one measurement.
    pub fn push_nanos(&mut self, nanos: u64) {
        self.samples.push(nanos);
    }

    /// How many measurements were taken.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether nothing has been measured yet.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Smallest measurement, in nanoseconds.
    pub fn min_nanos(&self) -> Option<u64> {
        self.samples.iter().copied().min()
    }

    /// Largest measurement, in nanoseconds.
    pub fn max_nanos(&self) -> Option<u64> {
        self.samples.iter().copied().max()
    }

    /// Arithmetic mean in nanoseconds, rounded down. Integer maths
    /// throughout: no float ever touches a timing number.
    pub fn mean_nanos(&self) -> Option<u64> {
        if self.samples.is_empty() {
            return None;
        }
        let total: u128 = self.samples.iter().map(|&value| u128::from(value)).sum();
        u64::try_from(total / self.samples.len() as u128).ok()
    }

    /// The measurement at `percentile` (0..=100), nearest-rank.
    pub fn percentile_nanos(&self, percentile: u32) -> Option<u64> {
        if self.samples.is_empty() || percentile > 100 {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        // Nearest-rank: ceil(p/100 * n), clamped into the slice.
        let rank = (sorted.len() as u128 * u128::from(percentile)).div_ceil(100);
        let index = usize::try_from(rank.max(1) - 1).ok()?;
        sorted.get(index.min(sorted.len() - 1)).copied()
    }

    /// A one-line `min/mean/p95/max` summary in microseconds.
    pub fn summary_micros(&self) -> String {
        let micros = |nanos: Option<u64>| nanos.map_or(0, |value| value / 1_000);
        format!(
            "n={} min={}us mean={}us p95={}us max={}us",
            self.len(),
            micros(self.min_nanos()),
            micros(self.mean_nanos()),
            micros(self.percentile_nanos(95)),
            micros(self.max_nanos())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{DecodedFrame, Durations, FrameSlot};
    use crate::nv12::Nv12Geometry;
    use std::time::Instant;
    use sub_time::{Rational, RationalTime};

    fn frame(pts_nanos: i64) -> DecodedFrame {
        let rate = Rational::new(1_000_000_000, 1).expect("nanoseconds is a valid rate");
        let geometry = Nv12Geometry::packed(2, 2).expect("2x2 is a valid picture");
        DecodedFrame {
            geometry,
            pts: RationalTime::new(pts_nanos, rate),
            y: vec![0; geometry.y_plane_len()],
            uv: vec![128; geometry.uv_plane_len()],
            arrived: Instant::now(),
        }
    }

    #[test]
    fn an_empty_slot_yields_nothing() {
        let slot = FrameSlot::new();
        assert!(!slot.has_frame());
        assert!(slot.take().is_none());
    }

    #[test]
    fn taking_a_frame_empties_the_slot() {
        let slot = FrameSlot::new();
        assert!(!slot.put(frame(0)), "the first frame displaces nothing");
        assert!(slot.has_frame());
        assert_eq!(slot.take().expect("a frame was put").pts.value(), 0);
        assert!(!slot.has_frame());
    }

    #[test]
    fn the_newest_frame_wins_and_the_drop_is_reported() {
        let slot = FrameSlot::new();
        assert!(!slot.put(frame(1)));
        assert!(slot.put(frame(2)), "the second frame displaces the first");
        assert!(slot.put(frame(3)), "the third displaces the second");
        let taken = slot.take().expect("a frame is waiting");
        assert_eq!(
            taken.pts.value(),
            3,
            "a late UI must be shown the newest picture, not a stale one"
        );
    }

    #[test]
    fn a_frame_keeps_its_pts_exact_in_nanoseconds() {
        // 1/24 s is not representable as a float without loss; as an exact
        // nanosecond count it round-trips.
        let taken = frame(41_666_667);
        assert_eq!(taken.pts.value(), 41_666_667);
        assert_eq!(taken.pts.rate().numerator(), 1_000_000_000);
    }

    #[test]
    fn an_empty_duration_set_has_no_statistics() {
        let durations = Durations::new();
        assert!(durations.is_empty());
        assert_eq!(durations.min_nanos(), None);
        assert_eq!(durations.mean_nanos(), None);
        assert_eq!(durations.percentile_nanos(95), None);
        assert!(durations.summary_micros().contains("n=0"));
    }

    #[test]
    fn statistics_are_exact_integers() {
        let mut durations = Durations::new();
        for value in [10u64, 20, 30, 40] {
            durations.push_nanos(value);
        }
        assert_eq!(durations.len(), 4);
        assert_eq!(durations.min_nanos(), Some(10));
        assert_eq!(durations.max_nanos(), Some(40));
        assert_eq!(durations.mean_nanos(), Some(25));
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut durations = Durations::new();
        for value in 1..=100u64 {
            durations.push_nanos(value);
        }
        assert_eq!(durations.percentile_nanos(0), Some(1));
        assert_eq!(durations.percentile_nanos(50), Some(50));
        assert_eq!(durations.percentile_nanos(95), Some(95));
        assert_eq!(durations.percentile_nanos(100), Some(100));
        assert_eq!(durations.percentile_nanos(101), None);
    }

    #[test]
    fn the_summary_reports_microseconds() {
        let mut durations = Durations::new();
        durations.push_nanos(1_500_000);
        assert!(
            durations.summary_micros().contains("min=1500us"),
            "{}",
            durations.summary_micros()
        );
    }
}
