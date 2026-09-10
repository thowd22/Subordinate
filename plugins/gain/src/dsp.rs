//! The gain DSP: a target level in decibels, approached one frame at a time.
//!
//! Kept apart from the component bindings so it can be unit-tested on the host
//! triple and read without any WIT in the way. Nothing here allocates, locks or
//! panics: it is a state variable and a multiply.

/// Below this level the gain is exactly zero rather than a very small number,
/// so a fader at its minimum is silence.
const SILENCE_DB: f32 = -60.0;

/// One frame's worth of smoothing state: the gain actually applied right now.
///
/// The target gain changes the instant a user moves a fader; applying that
/// jump directly to samples clicks. Instead each frame moves a fraction of the
/// way from the current gain to the target, so a step becomes an exponential
/// ramp whose time constant is the smoothing parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmoothedGain {
    /// The linear gain the last frame was multiplied by, or `None` before the
    /// first frame, when the target is taken up immediately rather than ramped
    /// from silence.
    current: Option<f32>,
}

impl Default for SmoothedGain {
    fn default() -> Self {
        Self::new()
    }
}

impl SmoothedGain {
    /// A fresh smoother that snaps to its first target.
    pub const fn new() -> Self {
        Self { current: None }
    }

    /// The gain currently applied, in linear amplitude, once a block has run.
    pub fn current(self) -> Option<f32> {
        self.current
    }

    /// Forgets the ramp, so the next block starts at its target. The host calls
    /// this after a seek, where interpolating across the cut means nothing.
    pub fn reset(&mut self) {
        self.current = None;
    }

    /// Converts decibels full scale to linear amplitude, with a floor at
    /// [`SILENCE_DB`].
    pub fn linear_gain(decibels: f32) -> f32 {
        if !decibels.is_finite() || decibels <= SILENCE_DB {
            return 0.0;
        }
        10.0_f32.powf(decibels / 20.0)
    }

    /// How much of the remaining distance to the target one frame covers.
    ///
    /// `smoothing_seconds` is the time constant: after that long, roughly 63%
    /// of a step has been travelled. Zero smoothing, or a rate that cannot
    /// resolve it, gives 1.0 — an immediate jump.
    fn coefficient(smoothing_seconds: f32, sample_rate: u32) -> f32 {
        if sample_rate == 0 || !smoothing_seconds.is_finite() || smoothing_seconds <= 0.0 {
            return 1.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let frames = smoothing_seconds * sample_rate as f32;
        if frames <= 1.0 {
            return 1.0;
        }
        1.0 / frames
    }

    /// Applies the gain to one block of interleaved samples, in place.
    ///
    /// `block.len()` must be a whole multiple of `channels`; a remainder is
    /// left untouched rather than read out of a frame that is not there.
    /// `channels` of zero leaves the block alone.
    pub fn process(
        &mut self,
        block: &mut [f32],
        channels: usize,
        sample_rate: u32,
        target_db: f32,
        smoothing_seconds: f32,
    ) {
        if channels == 0 {
            return;
        }
        let target = Self::linear_gain(target_db);
        let mut gain = self.current.unwrap_or(target);
        let coefficient = Self::coefficient(smoothing_seconds, sample_rate);

        for frame in block.chunks_exact_mut(channels) {
            gain += (target - gain) * coefficient;
            for sample in frame {
                *sample *= gain;
            }
        }

        // A ramp converges but never quite arrives; snapping once it is within
        // a sixteen-bit LSB of the target keeps a held fader from smoothing
        // forever.
        if (target - gain).abs() <= 1.0 / 32_768.0 {
            gain = target;
        }
        self.current = Some(gain);
    }
}

#[cfg(test)]
mod tests {
    use super::SmoothedGain;

    /// Two channels of ones: any change in the output is the gain.
    fn ones(frames: usize, channels: usize) -> Vec<f32> {
        vec![1.0; frames * channels]
    }

    #[test]
    fn zero_decibels_is_unity_and_the_floor_is_silence() {
        assert!((SmoothedGain::linear_gain(0.0) - 1.0).abs() < 1e-6);
        assert!((SmoothedGain::linear_gain(6.0) - 1.995_262).abs() < 1e-5);
        assert!((SmoothedGain::linear_gain(-6.0) - 0.501_187).abs() < 1e-5);
        assert_eq!(SmoothedGain::linear_gain(-60.0), 0.0);
        assert_eq!(SmoothedGain::linear_gain(-120.0), 0.0);
        assert_eq!(SmoothedGain::linear_gain(f32::NAN), 0.0);
    }

    #[test]
    fn the_first_block_starts_at_its_target_rather_than_ramping_from_silence() {
        let mut gain = SmoothedGain::new();
        let mut block = ones(4, 2);
        gain.process(&mut block, 2, 48_000, -6.0, 0.05);
        for sample in &block {
            assert!((sample - 0.501_187).abs() < 1e-4, "{sample}");
        }
    }

    #[test]
    fn no_smoothing_applies_the_new_target_from_the_first_frame() {
        let mut gain = SmoothedGain::new();
        let mut block = ones(2, 1);
        gain.process(&mut block, 1, 48_000, 0.0, 0.0);
        block = ones(2, 1);
        gain.process(&mut block, 1, 48_000, -6.0, 0.0);
        assert!((block[0] - 0.501_187).abs() < 1e-4, "{}", block[0]);
    }

    #[test]
    fn smoothing_ramps_toward_the_target_instead_of_stepping() {
        let mut gain = SmoothedGain::new();
        // Settle at unity.
        let mut block = ones(64, 1);
        gain.process(&mut block, 1, 48_000, 0.0, 0.01);
        assert_eq!(gain.current(), Some(1.0));

        // Now ask for silence with a 10 ms time constant at 48 kHz: the first
        // frame must move only a little, and the block must fall monotonically
        // without ever jumping straight to the target.
        let mut block = ones(480, 1);
        gain.process(&mut block, 1, 48_000, -60.0, 0.01);
        assert!(block[0] > 0.99, "{}", block[0]);
        assert!(block.windows(2).all(|pair| pair[1] < pair[0]));
        // One time constant covers about 63% of the way to zero.
        let last = block[block.len() - 1];
        assert!((0.30..0.40).contains(&last), "{last}");
    }

    #[test]
    fn a_held_target_settles_exactly_and_stops_moving() {
        let mut gain = SmoothedGain::new();
        let mut block = ones(8, 1);
        gain.process(&mut block, 1, 48_000, 0.0, 0.005);
        for _ in 0..64 {
            let mut block = ones(512, 1);
            gain.process(&mut block, 1, 48_000, -6.0, 0.005);
        }
        assert_eq!(gain.current(), Some(SmoothedGain::linear_gain(-6.0)));
    }

    #[test]
    fn every_channel_of_a_frame_gets_the_same_gain() {
        let mut gain = SmoothedGain::new();
        let mut block = vec![1.0, -1.0, 1.0, -1.0];
        gain.process(&mut block, 2, 48_000, -6.0, 0.01);
        assert!((block[0] + block[1]).abs() < 1e-9);
        assert!((block[2] + block[3]).abs() < 1e-9);
    }

    #[test]
    fn a_partial_frame_and_zero_channels_are_left_alone() {
        let mut gain = SmoothedGain::new();
        let mut block = vec![1.0, 1.0, 1.0];
        gain.process(&mut block, 2, 48_000, -60.0, 0.0);
        // The trailing half-frame is untouched.
        assert_eq!(block[2], 1.0);

        let mut block = vec![1.0, 1.0];
        let mut gain = SmoothedGain::new();
        gain.process(&mut block, 0, 48_000, -60.0, 0.0);
        assert_eq!(block, vec![1.0, 1.0]);
        assert_eq!(gain.current(), None);
    }

    #[test]
    fn a_reset_makes_the_next_block_start_at_its_target() {
        let mut gain = SmoothedGain::new();
        let mut block = ones(16, 1);
        gain.process(&mut block, 1, 48_000, 0.0, 0.05);
        gain.reset();
        assert_eq!(gain.current(), None);
        let mut block = ones(1, 1);
        gain.process(&mut block, 1, 48_000, -6.0, 0.05);
        assert!((block[0] - 0.501_187).abs() < 1e-4, "{}", block[0]);
    }

    #[test]
    fn a_zero_sample_rate_cannot_divide_by_zero() {
        let mut gain = SmoothedGain::new();
        let mut block = ones(4, 1);
        gain.process(&mut block, 1, 0, -6.0, 0.05);
        assert!(block.iter().all(|sample| sample.is_finite()));
    }
}
