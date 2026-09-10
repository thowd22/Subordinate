//! Audio scrubbing: short windowed grains around the playhead
//! (docs/PLAN.md §5.4).
//!
//! Dragging the playhead in an NLE is expected to make a sound. What is played
//! is not the timeline running at 1x — the playhead is not moving at 1x — but a
//! **grain**: a few tens of milliseconds of the mix taken from wherever the
//! drag has landed, played once, at a reduced gain so a drag is never as loud
//! as playback.
//!
//! Two halves, one thread each, exactly as the mixer has:
//!
//! - The **engine thread** owns a [`ScrubControl`]. It applies
//!   [`ScrubSettings`] — the enable toggle and the grain length — and asks for
//!   a grain every time the drag moves the playhead.
//! - The **audio callback** owns a [`ScrubPlayer`], attached to the mixer with
//!   [`crate::mixer::Mixer::with_scrub`]. It moves the mixer's transport to the
//!   grain's start, lets the mixer render the grain, and shapes what comes out.
//!
//! Nothing crosses between them but relaxed atomics: a request is one packed
//! store, the settings are one load each, and the callback neither locks nor
//! allocates.
//!
//! **No clicks.** A grain that started and stopped at full amplitude would
//! begin and end on a step, and a drag would be a burst of ticks. Every grain
//! is therefore multiplied by a raised-cosine (Hann) envelope that is exactly
//! zero on its first and last frame and rises and falls smoothly in between,
//! so both boundaries are silent whatever the samples underneath them are.
//! Between grains the mixer contributes silence and leaves the transport where
//! the last grain ended.
//!
//! ```
//! # fn main() -> sub_core::SubResult<()> {
//! use sub_audio::mixer::{MixGraphBuilder, MixerConfig, mixer};
//! use sub_audio::scrub::{ScrubSettings, scrub};
//! use sub_time::{Rational, RationalTime};
//!
//! let (control, player) = scrub(ScrubSettings::default(), 48_000)?;
//! let graph = MixGraphBuilder::new(48_000, 2).build()?;
//! let (_control, mut mixer) = mixer(graph, MixerConfig::default())?;
//! let mut mixer = mixer.with_scrub(player);
//!
//! // Engine thread: the drag landed here.
//! let rate = Rational::from_integer(48_000).expect("a valid rate");
//! control.grain_at(RationalTime::new(96_000, rate))?;
//!
//! // Audio callback: the grain plays from there, windowed.
//! let mut block = [0.0f32; 2 * 256];
//! mixer.process(&mut block);
//! assert_eq!(mixer.position_frames(), 96_000 + 256);
//! # Ok(())
//! # }
//! ```

use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use sub_core::{SubError, SubResult};
use sub_time::{Rational, RationalTime};

use crate::codes;
use crate::mixer::{MIN_GAIN_DB, frames_at, linear_gain};

/// The shortest grain the settings accept, in milliseconds. Anything shorter
/// is an envelope rather than a sound.
pub const MIN_GRAIN_MS: i64 = 5;

/// The longest grain the settings accept, in milliseconds. Past this a drag
/// stops tracking the pointer and turns into playback.
pub const MAX_GRAIN_MS: i64 = 500;

/// The grain length a fresh [`ScrubSettings`] uses, in milliseconds.
pub const DEFAULT_GRAIN_MS: i64 = 60;

/// How far scrubbing is turned down from playback, in decibels.
pub const DEFAULT_GAIN_DB: f64 = -9.0;

/// Bits of a packed request given to the grain's position, in frames. At
/// 48 kHz this covers about 186 years of timeline.
const POSITION_BITS: u32 = 48;

/// The frame positions a packed request can carry.
const POSITION_MASK: u64 = (1 << POSITION_BITS) - 1;

/// The milliseconds timebase the grain length is held at, so a length is an
/// exact rational rather than a float (docs/PLAN.md §5.1).
fn milliseconds() -> Rational {
    Rational::from_integer(1_000).expect("1000 is a valid timebase")
}

/// What scrubbing does when the playhead is dragged.
///
/// These are user settings: the toggle and the grain length are exposed in the
/// audio settings panel, and the gain is the fixed amount scrubbing is turned
/// down by unless a caller changes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrubSettings {
    /// Whether dragging the playhead makes a sound at all.
    pub enabled: bool,
    /// How long one grain lasts. Held as an exact time, never as a float.
    pub grain: RationalTime,
    /// How far a grain is turned down from the mix, in decibels.
    pub gain_db: f64,
}

impl Default for ScrubSettings {
    /// Scrubbing on, 60 ms grains, 9 dB down.
    fn default() -> Self {
        Self {
            enabled: true,
            grain: RationalTime::new(DEFAULT_GRAIN_MS, milliseconds()),
            gain_db: DEFAULT_GAIN_DB,
        }
    }
}

impl ScrubSettings {
    /// The same settings with the toggle set.
    #[must_use]
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The same settings with a grain length in whole milliseconds.
    #[must_use]
    pub fn with_grain_ms(mut self, milliseconds_length: i64) -> Self {
        self.grain = RationalTime::new(milliseconds_length, milliseconds());
        self
    }

    /// The grain length in whole milliseconds, rounded to the nearest.
    pub fn grain_ms(&self) -> i64 {
        self.grain.rescaled_to(milliseconds()).value()
    }

    /// Checks that these settings describe a grain the player can play.
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when the grain is not
    /// between [`MIN_GRAIN_MS`] and [`MAX_GRAIN_MS`] inclusive, and
    /// [`codes::INVALID_GAIN`] when the gain is not a finite level at or below
    /// unity — a scrub is quieter than playback, never louder.
    pub fn validate(&self) -> SubResult<()> {
        let (numerator, denominator) = self.grain.as_seconds_fraction();
        let too_short = numerator * i128::from(1_000) < denominator * i128::from(MIN_GRAIN_MS);
        let too_long = numerator * i128::from(1_000) > denominator * i128::from(MAX_GRAIN_MS);
        if denominator <= 0 || too_short || too_long {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "a scrub grain must last between 5 ms and 500 ms",
            )
            .with_detail("grain_ms", self.grain_ms()));
        }
        if !self.gain_db.is_finite() || !(MIN_GAIN_DB..=0.0).contains(&self.gain_db) {
            return Err(SubError::new(
                codes::INVALID_GAIN,
                "a scrub gain must be a finite level at or below 0 dB",
            )
            .with_detail("decibels", self.gain_db.to_string()));
        }
        Ok(())
    }

    /// The grain length in whole frames at `sample_rate`, never less than the
    /// two frames an envelope needs.
    ///
    /// # Errors
    ///
    /// Whatever [`ScrubSettings::validate`] returns, and
    /// [`sub_core::codes::INVALID_ARGUMENT`] when the sample rate is zero.
    pub fn grain_frames(&self, sample_rate: u32) -> SubResult<u64> {
        self.validate()?;
        let rate = Rational::from_integer(sample_rate).ok_or_else(|| {
            SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "a scrub player needs a non-zero sample rate",
            )
        })?;
        let frames = frames_at(self.grain, rate, "scrub grain length")?;
        Ok(frames.max(2))
    }

    /// The linear factor a grain is multiplied by.
    ///
    /// # Errors
    ///
    /// Whatever [`ScrubSettings::validate`] and [`linear_gain`] return.
    pub fn gain(&self) -> SubResult<f32> {
        self.validate()?;
        linear_gain(self.gain_db)
    }
}

/// The settings and the pending request, as the two threads share them.
///
/// Every field is a relaxed atomic: the callback only ever loads, the engine
/// only ever stores, and a load that misses a store by a block simply starts
/// the grain a block later.
#[derive(Debug)]
struct ScrubState {
    /// Whether scrubbing plays anything.
    enabled: AtomicBool,
    /// The grain length in frames at the sequence sample rate.
    grain_frames: AtomicU64,
    /// The linear gain a grain is played at, as `f32` bits.
    gain: AtomicU32,
    /// The last grain asked for: a 16-bit sequence number in the top bits and
    /// the start position in frames in the low [`POSITION_BITS`]. Packed into
    /// one word so the callback can never read a new sequence number against
    /// an old position.
    request: AtomicU64,
    /// The sequence sample rate positions are counted at.
    sample_rate: u32,
}

/// The engine thread's half of a scrub player.
///
/// Cheap to clone: every clone drives the same player.
#[derive(Debug, Clone)]
pub struct ScrubControl {
    /// The shared settings and request.
    state: Arc<ScrubState>,
}

/// The audio callback's half: the grain in progress and the envelope over it.
///
/// Attached to a mixer with [`crate::mixer::Mixer::with_scrub`]. Nothing here
/// locks, allocates or blocks.
#[derive(Debug)]
pub struct ScrubPlayer {
    /// The shared settings and request.
    state: Arc<ScrubState>,
    /// The sequence number of the last request started, so the same request is
    /// never played twice.
    last_seq: u64,
    /// Frames of the grain in progress already played.
    offset: u64,
    /// How long the grain in progress is, in frames.
    length: u64,
    /// The gain the grain in progress is played at, latched when it started so
    /// a settings change cannot step its amplitude part way through.
    gain: f32,
    /// Whether a grain is in progress at all.
    playing: bool,
}

/// What the player wants done with one stretch of a block.
///
/// Returned by [`ScrubPlayer::prepare`] and understood only by the mixer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrubSlice {
    /// Where to put the transport before rendering, when a new grain starts.
    pub(crate) seek: Option<u64>,
    /// How many frames of the requested run this slice covers. Never zero, and
    /// never more than was asked for, so a grain boundary always falls on a
    /// slice boundary.
    pub(crate) frames: usize,
    /// Whether the slice is silence: nothing to render, and the transport
    /// stays where it is.
    pub(crate) silent: bool,
}

/// A scrub player and the handle that drives it, at `sample_rate`.
///
/// # Errors
///
/// Whatever [`ScrubSettings::grain_frames`] and [`ScrubSettings::gain`]
/// return: a grain outside the accepted range, a gain above unity, or a zero
/// sample rate.
pub fn scrub(settings: ScrubSettings, sample_rate: u32) -> SubResult<(ScrubControl, ScrubPlayer)> {
    let state = Arc::new(ScrubState {
        enabled: AtomicBool::new(settings.enabled),
        grain_frames: AtomicU64::new(settings.grain_frames(sample_rate)?),
        gain: AtomicU32::new(settings.gain()?.to_bits()),
        request: AtomicU64::new(0),
        sample_rate,
    });
    let control = ScrubControl {
        state: Arc::clone(&state),
    };
    let player = ScrubPlayer {
        state,
        last_seq: 0,
        offset: 0,
        length: 0,
        gain: 0.0,
        playing: false,
    };
    Ok((control, player))
}

impl ScrubControl {
    /// Applies a whole set of settings.
    ///
    /// A grain already sounding is left alone: the change takes effect on the
    /// next grain, so turning a knob mid-drag cannot click.
    ///
    /// # Errors
    ///
    /// Whatever [`ScrubSettings::grain_frames`] and [`ScrubSettings::gain`]
    /// return. Nothing is applied when either fails.
    pub fn apply(&self, settings: ScrubSettings) -> SubResult<()> {
        let frames = settings.grain_frames(self.state.sample_rate)?;
        let gain = settings.gain()?;
        self.state.grain_frames.store(frames, Ordering::Relaxed);
        self.state.gain.store(gain.to_bits(), Ordering::Relaxed);
        self.state
            .enabled
            .store(settings.enabled, Ordering::Relaxed);
        Ok(())
    }

    /// Turns scrubbing on or off without touching the rest of the settings.
    pub fn set_enabled(&self, enabled: bool) {
        self.state.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether scrubbing plays anything.
    pub fn is_enabled(&self) -> bool {
        self.state.enabled.load(Ordering::Relaxed)
    }

    /// The grain length in force, in frames.
    pub fn grain_frames(&self) -> u64 {
        self.state.grain_frames.load(Ordering::Relaxed)
    }

    /// The linear gain in force.
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.state.gain.load(Ordering::Relaxed))
    }

    /// The sequence sample rate the player counts at.
    pub fn sample_rate(&self) -> u32 {
        self.state.sample_rate
    }

    /// Asks for one grain starting at `position`.
    ///
    /// The drag calls this every time it moves; a request that arrives while a
    /// grain is still sounding replaces it at the next block boundary, which is
    /// what makes a fast drag track the pointer.
    ///
    /// # Errors
    ///
    /// Returns [`codes::GRAPH_INVALID`] when the position is negative or not
    /// representable in frames at the sequence sample rate.
    pub fn grain_at(&self, position: RationalTime) -> SubResult<()> {
        let rate = Rational::from_integer(self.state.sample_rate).ok_or_else(|| {
            SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "a scrub player needs a non-zero sample rate",
            )
        })?;
        let frames = frames_at(position, rate, "scrub position")?.min(POSITION_MASK);
        let previous = self.state.request.load(Ordering::Relaxed);
        let seq = ((previous >> POSITION_BITS).wrapping_add(1)) & 0xFFFF;
        self.state
            .request
            .store((seq << POSITION_BITS) | frames, Ordering::Relaxed);
        Ok(())
    }
}

impl ScrubPlayer {
    /// Whether scrubbing is turned on. When it is off the mixer runs exactly
    /// as it does with no player attached.
    pub fn is_enabled(&self) -> bool {
        self.state.enabled.load(Ordering::Relaxed)
    }

    /// Whether a grain is sounding right now.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// The grain length in force, in frames.
    pub fn grain_frames(&self) -> u64 {
        self.state.grain_frames.load(Ordering::Relaxed)
    }

    /// Decides what to do with the next `frames` frames of a block.
    ///
    /// `None` means the player is not engaged and the mixer should render as
    /// it normally does. Real-time safe: two relaxed loads and no branch that
    /// can allocate.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a slice is never longer than the block it is cut from, which is a usize"
    )]
    pub(crate) fn prepare(&mut self, frames: usize) -> Option<ScrubSlice> {
        if !self.is_enabled() {
            // A grain left half played when the toggle went off is abandoned
            // rather than resumed later at the wrong amplitude.
            self.playing = false;
            return None;
        }
        let request = self.state.request.load(Ordering::Relaxed);
        let seq = request >> POSITION_BITS;
        let mut seek = None;
        if seq != self.last_seq {
            self.last_seq = seq;
            self.offset = 0;
            self.length = self.grain_frames().max(2);
            self.gain = f32::from_bits(self.state.gain.load(Ordering::Relaxed));
            self.playing = true;
            seek = Some(request & POSITION_MASK);
        }
        if !self.playing {
            return Some(ScrubSlice {
                seek: None,
                frames,
                silent: true,
            });
        }
        let left = self.length - self.offset;
        let frames = (left.min(frames as u64) as usize).max(1);
        Some(ScrubSlice {
            seek,
            frames,
            silent: false,
        })
    }

    /// Shapes `frames` rendered frames of the grain in place and advances it.
    ///
    /// The envelope is a raised cosine over the whole grain, so the first and
    /// last frame are exactly silent and everything between them rises and
    /// falls smoothly; the grain's gain is folded into the same multiply.
    ///
    /// Real-time safe: arithmetic on the block that was just rendered, no
    /// lock, no allocation, no table to look up.
    #[allow(
        clippy::cast_precision_loss,
        reason = "the envelope phase is a ratio of frame counts within one grain"
    )]
    pub(crate) fn shape(&mut self, block: &mut [f32], channels: usize, frames: usize) {
        let last = self.length.saturating_sub(1);
        for frame in 0..frames {
            let index = self.offset + frame as u64;
            let factor = if index == 0 || index >= last {
                0.0
            } else {
                let phase = TAU * (index as f32) / (last as f32);
                0.5f32.mul_add(-phase.cos(), 0.5) * self.gain
            };
            for sample in &mut block[frame * channels..(frame + 1) * channels] {
                *sample *= factor;
            }
        }
        self.offset += frames as u64;
        if self.offset >= self.length {
            self.playing = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_GRAIN_MS, MIN_GRAIN_MS, ScrubPlayer, ScrubSettings, scrub};
    use sub_time::{Rational, RationalTime};

    fn rate() -> Rational {
        Rational::from_integer(48_000).expect("a valid rate")
    }

    /// Plays one whole grain of a constant signal, returning the shaped mono
    /// frames.
    fn play_grain(player: &mut ScrubPlayer, block_frames: usize) -> Vec<f32> {
        let mut grain = Vec::new();
        while let Some(slice) = player.prepare(block_frames) {
            if slice.silent {
                break;
            }
            let mut block = vec![1.0f32; slice.frames];
            player.shape(&mut block, 1, slice.frames);
            grain.extend_from_slice(&block);
            if !player.is_playing() {
                break;
            }
        }
        grain
    }

    /// Whether a sample is silent. Written this way rather than as an equality
    /// because comparing floats for equality is what clippy is there to stop.
    fn is_silent(sample: f32) -> bool {
        sample.abs() < f32::EPSILON
    }

    #[test]
    fn default_settings_are_valid_and_quieter_than_playback() {
        let settings = ScrubSettings::default();
        settings.validate().expect("the defaults are playable");
        assert!(settings.enabled, "scrubbing is on out of the box");
        assert!(
            settings.gain().expect("a gain") < 1.0,
            "the linear gain is below unity"
        );
        assert_eq!(settings.grain_ms(), 60);
    }

    #[test]
    fn grain_length_is_bounded_at_both_ends() {
        let short = ScrubSettings::default().with_grain_ms(MIN_GRAIN_MS - 1);
        let long = ScrubSettings::default().with_grain_ms(MAX_GRAIN_MS + 1);
        for settings in [short, long] {
            let error = settings.validate().expect_err("out of range");
            assert_eq!(error.code, sub_core::codes::INVALID_ARGUMENT);
        }
        for length in [MIN_GRAIN_MS, MAX_GRAIN_MS] {
            ScrubSettings::default()
                .with_grain_ms(length)
                .validate()
                .expect("the bounds themselves are accepted");
        }
    }

    #[test]
    fn a_scrub_may_not_be_louder_than_playback() {
        let mut settings = ScrubSettings {
            gain_db: 3.0,
            ..ScrubSettings::default()
        };
        let error = settings.validate().expect_err("above unity");
        assert_eq!(error.code, crate::codes::INVALID_GAIN);
        settings.gain_db = f64::NAN;
        assert!(settings.validate().is_err(), "a gain must be finite");
    }

    #[test]
    fn grain_length_converts_to_frames_at_the_sequence_rate() {
        let settings = ScrubSettings::default().with_grain_ms(50);
        assert_eq!(settings.grain_frames(48_000).expect("frames"), 2_400);
        assert_eq!(settings.grain_frames(44_100).expect("frames"), 2_205);
        assert!(
            settings.grain_frames(0).is_err(),
            "a zero sample rate has no frames"
        );
    }

    #[test]
    fn nothing_sounds_until_a_grain_is_asked_for() {
        let (_control, mut player) = scrub(ScrubSettings::default(), 48_000).expect("a player");
        let slice = player.prepare(256).expect("engaged");
        assert!(slice.silent, "an idle scrub is silent");
        assert_eq!(
            slice.seek, None,
            "an idle scrub does not move the transport"
        );
        assert!(!player.is_playing());
    }

    #[test]
    fn a_request_seeks_the_transport_to_its_position() {
        let (control, mut player) = scrub(ScrubSettings::default(), 48_000).expect("a player");
        control
            .grain_at(RationalTime::new(96_000, rate()))
            .expect("a request");
        let slice = player.prepare(256).expect("engaged");
        assert_eq!(slice.seek, Some(96_000));
        assert!(!slice.silent);
        // Only the first slice of a grain seeks.
        player.shape(&mut vec![1.0; 256], 1, slice.frames);
        let next = player.prepare(256).expect("engaged");
        assert_eq!(next.seek, None);
    }

    #[test]
    fn a_negative_position_is_refused() {
        let (control, _player) = scrub(ScrubSettings::default(), 48_000).expect("a player");
        let error = control
            .grain_at(RationalTime::new(-1, rate()))
            .expect_err("a negative position");
        assert_eq!(error.code, crate::codes::GRAPH_INVALID);
    }

    #[test]
    fn a_grain_is_windowed_to_silence_at_both_boundaries() {
        let (control, mut player) =
            scrub(ScrubSettings::default().with_grain_ms(10), 48_000).expect("a player");
        control
            .grain_at(RationalTime::zero(rate()))
            .expect("a request");
        let grain = play_grain(&mut player, 64);
        assert_eq!(grain.len(), 480, "10 ms at 48 kHz");
        assert!(is_silent(grain[0]), "a grain starts from silence");
        assert!(
            is_silent(grain[grain.len() - 1]),
            "and ends in silence, so there is no step at either boundary"
        );
        let middle = grain[grain.len() / 2];
        assert!(middle > 0.0, "the middle of the grain is audible");
        // The envelope rises smoothly: no neighbouring pair steps by much.
        let biggest = grain
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            biggest < 0.01,
            "a windowed grain moves smoothly; biggest step was {biggest}"
        );
    }

    #[test]
    fn a_grain_never_reaches_the_gain_of_playback() {
        let (control, mut player) =
            scrub(ScrubSettings::default().with_grain_ms(20), 48_000).expect("a player");
        control
            .grain_at(RationalTime::zero(rate()))
            .expect("a request");
        let samples = play_grain(&mut player, 128);
        let peak = samples.iter().copied().fold(0.0f32, f32::max);
        let level = ScrubSettings::default().gain().expect("a gain");
        assert!(
            peak <= level + 1e-6,
            "a grain peaks at the scrub gain, not above it: {peak} > {level}"
        );
        assert!(peak > level * 0.9, "the envelope reaches close to its peak");
    }

    #[test]
    fn the_envelope_is_symmetric_about_the_middle() {
        let (control, mut player) =
            scrub(ScrubSettings::default().with_grain_ms(10), 48_000).expect("a player");
        control
            .grain_at(RationalTime::zero(rate()))
            .expect("a request");
        let grain = play_grain(&mut player, 480);
        let last = grain.len() - 1;
        for offset in 0..grain.len() / 2 {
            let (front, back) = (grain[offset], grain[last - offset]);
            assert!(
                (front - back).abs() < 1e-6,
                "the window is symmetric at {offset}: {front} vs {back}"
            );
        }
    }

    #[test]
    fn a_grain_ends_and_the_scrub_falls_silent() {
        let (control, mut player) =
            scrub(ScrubSettings::default().with_grain_ms(5), 48_000).expect("a player");
        control
            .grain_at(RationalTime::zero(rate()))
            .expect("a request");
        let grain = play_grain(&mut player, 1_024);
        assert_eq!(grain.len(), 240, "5 ms at 48 kHz");
        assert!(!player.is_playing(), "the grain is over");
        let slice = player.prepare(256).expect("engaged");
        assert!(slice.silent, "and nothing sounds until the next request");
    }

    #[test]
    fn a_new_request_restarts_the_grain_mid_flight() {
        let (control, mut player) =
            scrub(ScrubSettings::default().with_grain_ms(100), 48_000).expect("a player");
        control
            .grain_at(RationalTime::zero(rate()))
            .expect("a request");
        let first = player.prepare(256).expect("engaged");
        player.shape(&mut vec![1.0; 256], 1, first.frames);
        control
            .grain_at(RationalTime::new(48_000, rate()))
            .expect("a second request");
        let second = player.prepare(256).expect("engaged");
        assert_eq!(second.seek, Some(48_000), "the drag moved on");
        let mut block = vec![1.0; 256];
        player.shape(&mut block, 1, second.frames);
        assert!(
            is_silent(block[0]),
            "the restarted grain fades in from silence"
        );
    }

    #[test]
    fn the_toggle_disengages_the_player_entirely() {
        let (control, mut player) = scrub(ScrubSettings::default(), 48_000).expect("a player");
        control.set_enabled(false);
        control
            .grain_at(RationalTime::zero(rate()))
            .expect("a request");
        assert!(
            player.prepare(256).is_none(),
            "a disabled scrub leaves the mixer alone"
        );
        control.set_enabled(true);
        assert!(player.prepare(256).is_some(), "and re-engages when asked");
    }

    #[test]
    fn settings_are_applied_or_rejected_whole() {
        let (control, player) = scrub(ScrubSettings::default(), 48_000).expect("a player");
        control
            .apply(ScrubSettings::default().with_grain_ms(120))
            .expect("valid settings");
        assert_eq!(player.grain_frames(), 5_760);
        let error = control
            .apply(ScrubSettings::default().with_grain_ms(5_000))
            .expect_err("out of range");
        assert_eq!(error.code, sub_core::codes::INVALID_ARGUMENT);
        assert_eq!(
            player.grain_frames(),
            5_760,
            "a rejected change leaves the player as it was"
        );
        assert!(control.is_enabled());
        control
            .apply(ScrubSettings::default().with_enabled(false))
            .expect("valid settings");
        assert!(!control.is_enabled());
    }

    #[test]
    fn a_grain_is_shaped_the_same_whatever_the_block_length() {
        let settings = ScrubSettings::default().with_grain_ms(20);
        let mut grains = Vec::new();
        for block in [64usize, 199, 4_096] {
            let (control, mut player) = scrub(settings, 48_000).expect("a player");
            control
                .grain_at(RationalTime::zero(rate()))
                .expect("a request");
            grains.push(play_grain(&mut player, block));
        }
        assert_eq!(
            grains[0], grains[1],
            "block boundaries do not shape a grain"
        );
        assert_eq!(grains[1], grains[2]);
    }
}
