//! Level metering: peak and RMS per track and for the master bus
//! (docs/PLAN.md §5.4).
//!
//! A [`MeterBank`] is the only thing the audio callback tells the rest of the
//! program about levels. It is sized once, on the engine thread, and every
//! cell in it is a pair of atomics: the callback stores two relaxed words per
//! track per block and never reads back, so metering cannot lock the audio
//! thread, allocate on it, or make it wait for a reader. A reader takes a
//! torn-free snapshot of one cell at a time, which is all a meter needs — the
//! next block is a few milliseconds away.
//!
//! Levels are linear amplitudes, not decibels: the conversion belongs to
//! whoever draws the meter, and keeping the callback's side to a `max` and a
//! multiply-add keeps [`Mixer::process`](crate::mixer::Mixer::process) cheap.
//!
//! ```
//! use sub_audio::meter::{MeterBank, levels_of};
//!
//! let bank = MeterBank::new(2);
//! assert_eq!(bank.track(0).unwrap().peak, 0.0);
//!
//! // What the callback does once per block, per bus.
//! let levels = levels_of(&[0.5, -0.5, 0.5, -0.5]);
//! assert_eq!(levels.peak, 0.5);
//! assert_eq!(levels.rms, 0.5);
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

/// The peak and RMS of one bus over one block, as linear amplitudes.
///
/// `1.0` is full scale: a `peak` at or above it is a clipped bus, which is
/// what a meter's clip indicator lights on.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MeterLevels {
    /// The largest absolute sample in the block.
    pub peak: f32,
    /// The root mean square of the block, across every channel.
    pub rms: f32,
}

impl MeterLevels {
    /// A silent bus.
    pub const SILENT: Self = Self {
        peak: 0.0,
        rms: 0.0,
    };

    /// Levels with both fields set.
    #[must_use]
    pub const fn new(peak: f32, rms: f32) -> Self {
        Self { peak, rms }
    }

    /// Whether the peak reached or passed full scale.
    #[must_use]
    pub fn is_clipping(self) -> bool {
        self.peak >= 1.0
    }

    /// The louder of two measurements, field by field.
    #[must_use]
    pub fn max(self, other: Self) -> Self {
        Self {
            peak: self.peak.max(other.peak),
            rms: self.rms.max(other.rms),
        }
    }
}

/// Measures one interleaved block of any channel count.
///
/// The RMS is taken over every sample in the block, channels together, which
/// is what a single-bar meter shows. An empty block is silent rather than a
/// division by zero.
///
/// Real-time safe: one pass, no allocation, no branching on data.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "a block length is far below f64's exact integer range"
)]
#[allow(
    clippy::cast_possible_truncation,
    reason = "an RMS of f32 samples is inside f32's range by construction"
)]
pub fn levels_of(block: &[f32]) -> MeterLevels {
    if block.is_empty() {
        return MeterLevels::SILENT;
    }
    let mut peak = 0.0f32;
    // The sum is f64 so that a long block of quiet samples does not lose the
    // small ones; the cast back is the only place precision is spent.
    let mut sum = 0.0f64;
    for &sample in block {
        peak = peak.max(sample.abs());
        sum += f64::from(sample) * f64::from(sample);
    }
    let rms = (sum / block.len() as f64).sqrt();
    MeterLevels {
        peak,
        rms: rms as f32,
    }
}

/// One bus's cell: a peak and an RMS, each an `f32` in its bit pattern.
#[derive(Debug, Default)]
struct MeterCell {
    /// The most recent peak, as `f32::to_bits`.
    peak: AtomicU32,
    /// The most recent RMS, as `f32::to_bits`.
    rms: AtomicU32,
}

impl MeterCell {
    /// Publishes one block's levels. Two relaxed stores; never blocks.
    fn store(&self, levels: MeterLevels) {
        self.peak.store(levels.peak.to_bits(), Ordering::Relaxed);
        self.rms.store(levels.rms.to_bits(), Ordering::Relaxed);
    }

    /// Reads the cell from any thread.
    fn load(&self) -> MeterLevels {
        MeterLevels {
            peak: f32::from_bits(self.peak.load(Ordering::Relaxed)),
            rms: f32::from_bits(self.rms.load(Ordering::Relaxed)),
        }
    }
}

/// The levels of every track and of the master bus, shared between the audio
/// callback and whoever draws the meters.
///
/// The bank is allocated once with room for `track_capacity` tracks. A graph
/// with more tracks than that still plays: the tracks past the capacity are
/// simply not metered, which is a missing bar rather than a dropped block.
#[derive(Debug)]
pub struct MeterBank {
    /// One cell per track, indexed by the track's position in the graph.
    tracks: Box<[MeterCell]>,
    /// The master bus, measured after the master fader.
    master: MeterCell,
}

impl MeterBank {
    /// A silent bank with room for `track_capacity` tracks.
    #[must_use]
    pub fn new(track_capacity: usize) -> Self {
        let mut tracks = Vec::new();
        tracks.resize_with(track_capacity, MeterCell::default);
        Self {
            tracks: tracks.into_boxed_slice(),
            master: MeterCell::default(),
        }
    }

    /// How many tracks the bank can meter.
    #[must_use]
    pub fn track_capacity(&self) -> usize {
        self.tracks.len()
    }

    /// The levels of the track at `index`, or `None` when the bank has no
    /// cell for it.
    #[must_use]
    pub fn track(&self, index: usize) -> Option<MeterLevels> {
        self.tracks.get(index).map(MeterCell::load)
    }

    /// The levels of the master bus, after the master fader.
    #[must_use]
    pub fn master(&self) -> MeterLevels {
        self.master.load()
    }

    /// Publishes the levels of the track at `index`, ignoring an index the
    /// bank has no room for.
    ///
    /// Real-time safe: two relaxed stores and a bounds check.
    pub fn publish_track(&self, index: usize, levels: MeterLevels) {
        if let Some(cell) = self.tracks.get(index) {
            cell.store(levels);
        }
    }

    /// Publishes the master bus's levels. Real-time safe.
    pub fn publish_master(&self, levels: MeterLevels) {
        self.master.store(levels);
    }

    /// Silences every cell, for when playback stops.
    pub fn silence(&self) {
        for cell in &self.tracks {
            cell.store(MeterLevels::SILENT);
        }
        self.master.store(MeterLevels::SILENT);
    }
}

impl Default for MeterBank {
    /// A bank with no track cells, which meters the master only.
    fn default() -> Self {
        Self::new(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{MeterBank, MeterLevels, levels_of};

    #[test]
    fn an_empty_block_is_silent() {
        assert_eq!(levels_of(&[]), MeterLevels::SILENT);
    }

    #[test]
    fn peak_is_the_largest_magnitude_either_way() {
        let levels = levels_of(&[0.25, -0.75, 0.5]);
        assert!(
            (levels.peak - 0.75).abs() < 1e-6,
            "peak was {}",
            levels.peak
        );
    }

    #[test]
    fn rms_of_a_square_wave_is_its_amplitude() {
        let block: Vec<f32> = (0..1_000)
            .map(|index| if index % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let levels = levels_of(&block);
        assert!((levels.rms - 0.5).abs() < 1e-6, "rms was {}", levels.rms);
    }

    #[test]
    fn rms_of_a_sine_is_the_amplitude_over_root_two() {
        let block: Vec<f32> = (0..4_800)
            .map(|index| {
                #[allow(clippy::cast_precision_loss, reason = "a small test index")]
                let phase = index as f32 / 4_800.0 * std::f32::consts::TAU * 100.0;
                phase.sin()
            })
            .collect();
        let levels = levels_of(&block);
        let expected = 1.0 / 2.0f32.sqrt();
        assert!(
            (levels.rms - expected).abs() < 1e-3,
            "rms was {}",
            levels.rms
        );
    }

    #[test]
    fn full_scale_and_beyond_counts_as_clipping() {
        assert!(!MeterLevels::new(0.999, 0.5).is_clipping());
        assert!(MeterLevels::new(1.0, 0.5).is_clipping());
        assert!(MeterLevels::new(1.4, 0.5).is_clipping());
    }

    #[test]
    fn max_takes_the_louder_of_each_field() {
        let louder = MeterLevels::new(0.2, 0.9).max(MeterLevels::new(0.8, 0.1));
        assert_eq!(louder, MeterLevels::new(0.8, 0.9));
    }

    #[test]
    fn a_bank_starts_silent_and_round_trips_what_is_published() {
        let bank = MeterBank::new(3);
        assert_eq!(bank.track_capacity(), 3);
        assert_eq!(bank.master(), MeterLevels::SILENT);
        assert_eq!(bank.track(0), Some(MeterLevels::SILENT));
        assert_eq!(bank.track(3), None, "a track past the capacity has no cell");

        bank.publish_track(1, MeterLevels::new(0.5, 0.25));
        bank.publish_master(MeterLevels::new(1.0, 0.75));
        assert_eq!(bank.track(1), Some(MeterLevels::new(0.5, 0.25)));
        assert_eq!(bank.master(), MeterLevels::new(1.0, 0.75));
        assert_eq!(bank.track(0), Some(MeterLevels::SILENT));
    }

    #[test]
    fn publishing_past_the_capacity_is_ignored_rather_than_panicking() {
        let bank = MeterBank::new(1);
        bank.publish_track(9, MeterLevels::new(1.0, 1.0));
        assert_eq!(bank.track(0), Some(MeterLevels::SILENT));
    }

    #[test]
    fn silence_clears_every_cell() {
        let bank = MeterBank::new(2);
        bank.publish_track(0, MeterLevels::new(0.5, 0.5));
        bank.publish_track(1, MeterLevels::new(0.5, 0.5));
        bank.publish_master(MeterLevels::new(0.5, 0.5));
        bank.silence();
        assert_eq!(bank.track(0), Some(MeterLevels::SILENT));
        assert_eq!(bank.track(1), Some(MeterLevels::SILENT));
        assert_eq!(bank.master(), MeterLevels::SILENT);
    }

    #[test]
    fn a_default_bank_meters_the_master_only() {
        let bank = MeterBank::default();
        assert_eq!(bank.track_capacity(), 0);
        assert_eq!(bank.track(0), None);
        bank.publish_master(MeterLevels::new(0.1, 0.1));
        assert_eq!(bank.master(), MeterLevels::new(0.1, 0.1));
    }
}
