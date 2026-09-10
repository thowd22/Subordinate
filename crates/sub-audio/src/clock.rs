//! The audio clock: the playback master.
//!
//! Sync is defined by audio (docs/PLAN.md §5.4). The output callback is the
//! only thing in the editor that runs on a real, unforgiving clock — the
//! device pulls a block of frames whether or not anything else is ready — so
//! the position it has reached is the truth about where playback is, and the
//! video scheduler picks the frame that covers it rather than running a clock
//! of its own.
//!
//! [`AudioClock`] is how that position leaves the callback. It holds two
//! relaxed atomics the callback stores into once per block:
//!
//! - **rendered**: the mixer transport position after the block, in frames at
//!   the sequence sample rate. It counts samples the callback has actually
//!   rendered, not wall-clock time.
//! - **latency**: the frames of that position that are still in flight —
//!   written into the device's buffer but not yet heard.
//!
//! The audible playhead is the difference, so a reader gets the time the
//! listener is hearing rather than the time the callback has run ahead to.
//! Both are plain relaxed stores: no lock, no allocation, nothing that can
//! block the audio thread.
//!
//! ```
//! use std::sync::Arc;
//!
//! use sub_audio::clock::AudioClock;
//! use sub_time::{Rational, RationalTime};
//!
//! let clock = Arc::new(AudioClock::new(48_000).unwrap());
//! assert_eq!(clock.position(), None);
//!
//! // One 512-frame block rendered, all of it still in the device buffer.
//! clock.publish(512, 512);
//! assert_eq!(clock.position(), Some(RationalTime::zero(clock.rate())));
//!
//! // A second block: the first one is audible now.
//! clock.publish(1_024, 512);
//! let rate = Rational::from_integer(48_000).unwrap();
//! assert_eq!(clock.position(), Some(RationalTime::new(512, rate)));
//! ```

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sub_core::{SubError, SubResult};
use sub_time::{Rational, RationalTime};

/// Where the audio output has got to, as the callback publishes it.
///
/// A clock is shared between the audio thread, which only ever stores into it,
/// and any number of readers, which only ever load. Every access is a relaxed
/// atomic: the numbers are a clock reading, not a handshake, and a reader that
/// sees a block-old value simply reads the playhead a block late.
#[derive(Debug)]
pub struct AudioClock {
    /// The sequence sample rate the positions are counted at.
    sample_rate: u32,
    /// Mixer transport frames rendered, as of the last block.
    rendered: AtomicU64,
    /// How many of those frames have not been heard yet.
    latency: AtomicU64,
    /// Whether the callback has published anything since the last reset.
    running: AtomicBool,
}

impl AudioClock {
    /// A clock counting at `sample_rate`, with nothing rendered yet.
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when the sample rate is
    /// zero, which is not a rate anything can be timed against.
    pub fn new(sample_rate: u32) -> SubResult<Self> {
        if sample_rate == 0 {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "an audio clock needs a non-zero sample rate",
            ));
        }
        Ok(Self {
            sample_rate,
            rendered: AtomicU64::new(0),
            latency: AtomicU64::new(0),
            running: AtomicBool::new(false),
        })
    }

    /// The sequence sample rate positions are counted at.
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The sample rate as a timebase, for [`AudioClock::position`].
    ///
    /// # Panics
    ///
    /// Never: the rate was checked to be non-zero when the clock was built.
    pub fn rate(&self) -> Rational {
        Rational::from_integer(self.sample_rate).expect("a clock's sample rate is non-zero")
    }

    /// Publishes what one callback did: the transport position it left the
    /// mixer at, and how many of those frames are still in the device buffer.
    ///
    /// Real-time safe — three relaxed stores, no lock and no allocation — so
    /// it is called from the audio callback itself.
    pub fn publish(&self, rendered_frames: u64, latency_frames: u64) {
        self.rendered.store(rendered_frames, Ordering::Relaxed);
        self.latency.store(latency_frames, Ordering::Relaxed);
        self.running.store(true, Ordering::Relaxed);
    }

    /// Forgets everything published so far.
    ///
    /// The transport calls this when it seeks or stops the stream, so a
    /// reading left over from the old position cannot yank the playhead
    /// before the callback has rendered a block at the new one.
    pub fn reset(&self) {
        self.running.store(false, Ordering::Relaxed);
        self.rendered.store(0, Ordering::Relaxed);
        self.latency.store(0, Ordering::Relaxed);
    }

    /// Whether the callback has published a block since the last reset.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Transport frames the callback has rendered, as of the last block.
    pub fn rendered_frames(&self) -> u64 {
        self.rendered.load(Ordering::Relaxed)
    }

    /// How many rendered frames have not been heard yet.
    pub fn latency_frames(&self) -> u64 {
        self.latency.load(Ordering::Relaxed)
    }

    /// The audible playhead in frames: rendered minus what is still in flight.
    ///
    /// `None` until the callback has published a block, because before that
    /// there is no audio position to follow.
    pub fn position_frames(&self) -> Option<u64> {
        self.is_running()
            .then(|| self.rendered_frames().saturating_sub(self.latency_frames()))
    }

    /// The audible playhead as an exact time at the sequence sample rate.
    ///
    /// This is the master playback position: the video scheduler picks the
    /// frame that covers it (docs/PLAN.md §5.4).
    pub fn position(&self) -> Option<RationalTime> {
        let frames = self.position_frames()?;
        Some(RationalTime::new(
            i64::try_from(frames).unwrap_or(i64::MAX),
            self.rate(),
        ))
    }
}

/// Converts `frames` at `from` into whole frames at `to`, rounding down.
///
/// The callback uses it to express a block of device frames as the sequence
/// frames they will take to play. Integer arithmetic only: it runs on the
/// audio thread, and a rate ratio held in floating point would drift.
pub(crate) fn convert_frames(frames: u64, from: u32, to: u32) -> u64 {
    if from == to || from == 0 {
        return frames;
    }
    let converted = u128::from(frames) * u128::from(to) / u128::from(from);
    u64::try_from(converted).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_needs_a_real_sample_rate() {
        let error = AudioClock::new(0).expect_err("zero is not a sample rate");
        assert_eq!(error.code, sub_core::codes::INVALID_ARGUMENT);
    }

    #[test]
    fn a_fresh_clock_has_no_position() {
        let clock = AudioClock::new(48_000).expect("a clock");
        assert!(!clock.is_running());
        assert_eq!(clock.position_frames(), None);
        assert_eq!(clock.position(), None);
    }

    #[test]
    fn the_playhead_is_what_has_been_heard_not_what_has_been_rendered() {
        let clock = AudioClock::new(48_000).expect("a clock");
        clock.publish(4_800, 480);
        assert_eq!(clock.position_frames(), Some(4_320));
        assert_eq!(
            clock.position(),
            Some(RationalTime::new(4_320, clock.rate()))
        );
    }

    #[test]
    fn latency_longer_than_what_was_rendered_reads_as_zero() {
        let clock = AudioClock::new(48_000).expect("a clock");
        clock.publish(256, 1_024);
        assert_eq!(clock.position_frames(), Some(0));
    }

    #[test]
    fn a_reset_clock_reports_no_position_again() {
        let clock = AudioClock::new(48_000).expect("a clock");
        clock.publish(4_800, 480);
        clock.reset();
        assert!(!clock.is_running());
        assert_eq!(clock.position(), None);
        assert_eq!(clock.rendered_frames(), 0);
    }

    #[test]
    fn frames_convert_between_rates_without_floating_point() {
        assert_eq!(convert_frames(512, 48_000, 48_000), 512);
        assert_eq!(convert_frames(441, 44_100, 48_000), 480);
        assert_eq!(convert_frames(480, 48_000, 44_100), 441);
        // A block that is not a whole number of frames at the other rate
        // rounds down, so the reported latency is never longer than it is.
        assert_eq!(convert_frames(1, 44_100, 48_000), 1);
    }
}
