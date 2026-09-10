//! Integer-only sample statistics for the benchmark harness.
//!
//! Every measurement is an exact count of nanoseconds and every derived rate
//! is an integer, in keeping with the project rule that timing values are
//! never floats: a rate is reported in milli-frames per second, so 24.5 fps
//! reads as `24_500`.

use serde::{Deserialize, Serialize};

/// Nanoseconds in one second, as the numerator of a milli-fps division.
const MILLI_FPS_NUMERATOR: u128 = 1_000_000_000_000;

/// A collected series of durations, in nanoseconds.
#[derive(Debug, Default, Clone)]
pub struct Samples {
    nanos: Vec<u64>,
}

impl Samples {
    /// An empty series.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one measurement.
    pub fn push(&mut self, nanos: u64) {
        self.nanos.push(nanos);
    }

    /// How many measurements were recorded.
    pub fn len(&self) -> usize {
        self.nanos.len()
    }

    /// True when nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.nanos.is_empty()
    }

    /// The summary of the series, or `None` when nothing was recorded.
    pub fn summary(&self) -> Option<Stats> {
        if self.nanos.is_empty() {
            return None;
        }
        let mut sorted = self.nanos.clone();
        sorted.sort_unstable();
        let total: u128 = sorted.iter().copied().map(u128::from).sum();
        let count = u64::try_from(sorted.len()).unwrap_or(u64::MAX);
        let mean = u64::try_from(total / u128::from(count)).unwrap_or(u64::MAX);
        Some(Stats {
            count,
            mean_nanos: mean,
            min_nanos: sorted[0],
            p50_nanos: percentile(&sorted, 50),
            p95_nanos: percentile(&sorted, 95),
            max_nanos: sorted[sorted.len() - 1],
        })
    }
}

/// The nearest-rank percentile of an ascending series.
///
/// Rank is `ceil(len * p / 100)`, clamped into the slice, so `p95` of twenty
/// samples is the nineteenth and `p100` is the largest. The slice must not be
/// empty.
fn percentile(sorted: &[u64], p: u64) -> u64 {
    let len = u64::try_from(sorted.len()).unwrap_or(u64::MAX);
    let rank = (len * p).div_ceil(100).max(1);
    let index = usize::try_from(rank - 1).unwrap_or(0).min(sorted.len() - 1);
    sorted[index]
}

/// What one series of durations came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// How many measurements went into the summary.
    pub count: u64,
    /// Arithmetic mean, truncated to whole nanoseconds.
    pub mean_nanos: u64,
    /// Fastest measurement.
    pub min_nanos: u64,
    /// Median.
    pub p50_nanos: u64,
    /// 95th percentile, nearest rank.
    pub p95_nanos: u64,
    /// Slowest measurement.
    pub max_nanos: u64,
}

/// `frames` frames in `nanos` nanoseconds as milli-frames per second, or
/// `None` when no time elapsed.
pub fn rate_milli_fps(frames: u64, nanos: u64) -> Option<u64> {
    if nanos == 0 {
        return None;
    }
    u64::try_from(u128::from(frames) * MILLI_FPS_NUMERATOR / u128::from(nanos)).ok()
}

/// A milli-fps value as the `12.345` text the summary prints.
pub fn format_milli_fps(milli_fps: Option<u64>) -> String {
    match milli_fps {
        Some(value) => format!("{}.{:03}", value / 1_000, value % 1_000),
        None => "n/a".to_owned(),
    }
}

/// A nanosecond count as the `12.345 ms` text the summary prints.
pub fn format_millis(nanos: u64) -> String {
    format!("{}.{:03} ms", nanos / 1_000_000, (nanos / 1_000) % 1_000)
}

#[cfg(test)]
mod tests {
    use super::{Samples, format_milli_fps, format_millis, rate_milli_fps};

    fn samples(values: &[u64]) -> Samples {
        let mut samples = Samples::new();
        for value in values {
            samples.push(*value);
        }
        samples
    }

    #[test]
    fn an_empty_series_has_no_summary() {
        assert!(Samples::new().is_empty());
        assert_eq!(Samples::new().summary(), None);
    }

    #[test]
    fn percentiles_are_nearest_rank_and_order_independent() {
        let stats = samples(&[50, 10, 30, 100, 20, 40, 90, 60, 80, 70])
            .summary()
            .expect("ten samples summarise");
        assert_eq!(stats.count, 10);
        assert_eq!(stats.min_nanos, 10);
        assert_eq!(stats.max_nanos, 100);
        // Rank ceil(10 * 50 / 100) = 5 -> the fifth smallest.
        assert_eq!(stats.p50_nanos, 50);
        // Rank ceil(10 * 95 / 100) = 10 -> the largest.
        assert_eq!(stats.p95_nanos, 100);
        assert_eq!(stats.mean_nanos, 55);
    }

    #[test]
    fn one_sample_is_every_percentile() {
        let stats = samples(&[7]).summary().expect("one sample summarises");
        assert_eq!(stats.p50_nanos, 7);
        assert_eq!(stats.p95_nanos, 7);
        assert_eq!(stats.max_nanos, 7);
    }

    #[test]
    fn a_mean_of_forty_milliseconds_is_twenty_five_fps() {
        let stats = samples(&[40_000_000, 40_000_000])
            .summary()
            .expect("two samples summarise");
        assert_eq!(rate_milli_fps(1, stats.mean_nanos), Some(25_000));
    }

    #[test]
    fn a_zero_duration_reports_no_rate_instead_of_dividing_by_zero() {
        assert_eq!(rate_milli_fps(1, 0), None);
        assert_eq!(rate_milli_fps(60, 0), None);
    }

    #[test]
    fn sustained_rate_is_frames_over_wall_time() {
        // 60 frames in 2 s is exactly 30 fps.
        assert_eq!(rate_milli_fps(60, 2_000_000_000), Some(30_000));
    }

    #[test]
    fn rates_and_durations_print_with_three_decimals() {
        assert_eq!(format_milli_fps(Some(30_500)), "30.500");
        assert_eq!(format_milli_fps(Some(999)), "0.999");
        assert_eq!(format_milli_fps(None), "n/a");
        assert_eq!(format_millis(12_345_678), "12.345 ms");
        assert_eq!(format_millis(999), "0.000 ms");
    }
}
