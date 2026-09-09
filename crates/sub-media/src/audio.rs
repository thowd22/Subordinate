//! Audio that lives inside a video file: interleaved `f32` PCM off the same
//! GStreamer pipeline that decodes the pictures (decision-4, docs/PLAN.md §5.4).
//!
//! A video file is demuxed once. [`Decoder`](crate::Decoder) therefore carries
//! an audio branch — `audioconvert` into a second `appsink` — beside its video
//! branch, and hands the decoded samples out as [`AudioBlock`]s. Audio-only
//! files never come through here; `sub-audio` reads those with symphonia.
//!
//! Nothing in this module runs on the real-time audio callback: blocks are
//! decoded ahead of playback, so the copy and the fold here are free to
//! allocate.
//!
//! # Timestamps
//!
//! A block's [`start`](AudioBlock::start) is an exact [`RationalTime`] at the
//! source sample rate — one unit is one audio frame — seeded from the first
//! buffer's presentation timestamp and then advanced by the frames actually
//! delivered. Positions are therefore sample-accurate and contiguous rather
//! than re-derived from each buffer's own nanosecond timestamp, which a
//! container has already rounded.
//!
//! # Downmix gains
//!
//! Blocks are stereo by default, whatever the source carries, because the MVP
//! mixer bus is stereo. The fold is a plain gain matrix, one row per source
//! channel position:
//!
//! | Source channel | Left | Right |
//! |----------------|------|-------|
//! | mono | 1.0 | 1.0 |
//! | front left / front right | 1.0 | 1.0 |
//! | front centre | [`CENTER_DOWNMIX_GAIN`] | [`CENTER_DOWNMIX_GAIN`] |
//! | rear or side left / right | [`SURROUND_DOWNMIX_GAIN`] | [`SURROUND_DOWNMIX_GAIN`] |
//! | LFE | [`LFE_DOWNMIX_GAIN`] | [`LFE_DOWNMIX_GAIN`] |
//! | anything else | 0.0 | 0.0 |
//!
//! That is the ITU-R BS.775 fold: the centre and the surrounds come in 3 dB
//! down and the LFE is dropped, which is what a stereo monitor path expects.
//! The gains are not normalised afterwards, so a hot 5.1 source can fold past
//! ±1.0; holding the front pair at unity is worth more than guaranteed
//! headroom, and the mixer is where a limiter belongs.
//!
//! A caller that wants the source channels untouched asks for
//! [`AudioChannels::Source`] and does its own routing.

use gstreamer_audio::AudioChannelPosition;
use sub_time::{Rational, RationalTime, Rounding};

/// Gain the front centre channel is folded into both outputs with: −3 dB.
pub const CENTER_DOWNMIX_GAIN: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Gain a rear or side channel is folded into its own side with: −3 dB.
pub const SURROUND_DOWNMIX_GAIN: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Gain the LFE channel is folded in with: it is dropped.
pub const LFE_DOWNMIX_GAIN: f32 = 0.0;

/// The largest channel count this decoder will fold or deliver.
///
/// Well past 7.1; a stream claiming more is malformed, or is a format the
/// editor has no business playing.
pub const MAX_CHANNELS: u16 = 64;

/// What a source channel is for, as far as the stereo fold is concerned.
///
/// This is the part of a GStreamer channel position that changes a gain;
/// every other position (height channels, wide channels, unknown ones) folds
/// to silence and is reported as [`ChannelRole::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelRole {
    /// The only channel of a mono stream.
    Mono,
    /// Front left.
    FrontLeft,
    /// Front right.
    FrontRight,
    /// Front centre.
    FrontCenter,
    /// Low-frequency effects.
    Lfe,
    /// Rear or side left.
    SurroundLeft,
    /// Rear or side right.
    SurroundRight,
    /// A position the stereo fold has no gain for.
    Other,
}

impl ChannelRole {
    /// Maps a GStreamer channel position onto the roles the fold knows.
    fn from_position(position: AudioChannelPosition) -> Self {
        match position {
            AudioChannelPosition::Mono => Self::Mono,
            AudioChannelPosition::FrontLeft => Self::FrontLeft,
            AudioChannelPosition::FrontRight => Self::FrontRight,
            AudioChannelPosition::FrontCenter => Self::FrontCenter,
            AudioChannelPosition::Lfe1 | AudioChannelPosition::Lfe2 => Self::Lfe,
            AudioChannelPosition::RearLeft | AudioChannelPosition::SideLeft => Self::SurroundLeft,
            AudioChannelPosition::RearRight | AudioChannelPosition::SideRight => {
                Self::SurroundRight
            }
            _ => Self::Other,
        }
    }

    /// The `(left, right)` gains this role is folded into the stereo pair with.
    pub fn stereo_gains(self) -> (f32, f32) {
        match self {
            Self::Mono => (1.0, 1.0),
            Self::FrontLeft => (1.0, 0.0),
            Self::FrontRight => (0.0, 1.0),
            Self::FrontCenter => (CENTER_DOWNMIX_GAIN, CENTER_DOWNMIX_GAIN),
            Self::Lfe => (LFE_DOWNMIX_GAIN, LFE_DOWNMIX_GAIN),
            Self::SurroundLeft => (SURROUND_DOWNMIX_GAIN, 0.0),
            Self::SurroundRight => (0.0, SURROUND_DOWNMIX_GAIN),
            Self::Other => (0.0, 0.0),
        }
    }
}

/// The channel layout of a decoded audio stream.
///
/// Only the layouts the fold treats specially are named; anything else is
/// [`ChannelLayout::Other`] and keeps its first two channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelLayout {
    /// One channel.
    Mono,
    /// Front left and front right.
    Stereo,
    /// Front left, front right, front centre, LFE and a surround pair.
    Surround5_1,
    /// Any other channel count or set of positions.
    Other,
}

impl ChannelLayout {
    /// A short stable name, as it appears in logs and error details.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mono => "mono",
            Self::Stereo => "stereo",
            Self::Surround5_1 => "5.1",
            Self::Other => "other",
        }
    }

    /// Classifies a set of channel roles, in stream order.
    fn from_roles(roles: &[ChannelRole]) -> Self {
        match roles {
            [ChannelRole::Mono | ChannelRole::FrontCenter] => Self::Mono,
            [ChannelRole::FrontLeft, ChannelRole::FrontRight] => Self::Stereo,
            [
                ChannelRole::FrontLeft,
                ChannelRole::FrontRight,
                ChannelRole::FrontCenter,
                ChannelRole::Lfe,
                ChannelRole::SurroundLeft,
                ChannelRole::SurroundRight,
            ] => Self::Surround5_1,
            _ => Self::Other,
        }
    }
}

/// How many channels a [`Decoder`](crate::Decoder) delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AudioChannels {
    /// Fold whatever the source carries down to stereo with the documented
    /// gains. This is what the MVP mixer bus takes.
    #[default]
    StereoDownmix,
    /// Deliver the source channels untouched, in stream order.
    Source,
}

/// The shape of the audio a decoder is delivering.
///
/// The sample rate is the source's own; nothing here resamples — that happens
/// on the way into the mixer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFormat {
    /// Sample rate in hertz, unchanged from the source.
    pub sample_rate: u32,
    /// Channel count the source carries.
    pub source_channels: u16,
    /// Channel layout the source carries.
    pub source_layout: ChannelLayout,
    /// Channel count each [`AudioBlock`] carries: two when folding, otherwise
    /// [`AudioFormat::source_channels`].
    pub channels: u16,
}

impl AudioFormat {
    /// The rate every [`RationalTime`] from this stream is expressed at: one
    /// unit per audio frame.
    ///
    /// # Panics
    ///
    /// Never: a zero sample rate is rejected when the stream is negotiated.
    pub fn frame_rate(&self) -> Rational {
        Rational::new(self.sample_rate, 1).expect("a negotiated sample rate is non-zero")
    }

    /// Whether the source is being folded rather than passed through.
    pub fn is_downmixed(&self) -> bool {
        self.channels != self.source_channels
    }
}

/// One run of decoded audio frames, interleaved, borrowed from the decoder.
///
/// The samples live in the decoder's own buffer, which is reused block after
/// block, so a caller that keeps them copies them out.
#[derive(Debug)]
pub struct AudioBlock<'a> {
    /// Position of the first frame, exact at the source sample rate.
    pub start: RationalTime,
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Channel count; `samples.len()` is `frames * channels`.
    pub channels: u16,
    /// Interleaved samples, one frame at a time.
    pub samples: &'a [f32],
}

impl AudioBlock<'_> {
    /// Number of audio frames in this block.
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }

    /// Position just past the last frame, exact at the source sample rate.
    pub fn end(&self) -> RationalTime {
        let frames = i64::try_from(self.frames()).unwrap_or(i64::MAX);
        RationalTime::new(self.start.value().saturating_add(frames), self.start.rate())
    }
}

/// The gain matrix that folds one source layout into the stereo pair: one
/// `(left, right)` row per source channel, in stream order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StereoDownmix {
    gains: Vec<(f32, f32)>,
}

impl StereoDownmix {
    /// Builds the matrix for a set of channel roles, in stream order.
    pub(crate) fn from_roles(roles: &[ChannelRole]) -> Self {
        Self {
            gains: roles.iter().map(|role| role.stereo_gains()).collect(),
        }
    }

    /// The `(left, right)` gains, one row per source channel.
    #[cfg(test)]
    pub(crate) fn gains(&self) -> &[(f32, f32)] {
        &self.gains
    }

    /// Folds `input` — interleaved, one frame per matrix row — into `output`,
    /// which is cleared first and left holding interleaved stereo frames.
    pub(crate) fn apply(&self, input: &[f32], output: &mut Vec<f32>) {
        let channels = self.gains.len();
        output.clear();
        if channels == 0 {
            return;
        }
        output.reserve(input.len() / channels * 2);
        for frame in input.chunks_exact(channels) {
            let mut left = 0.0_f32;
            let mut right = 0.0_f32;
            for (sample, (gain_left, gain_right)) in frame.iter().zip(&self.gains) {
                left += sample * gain_left;
                right += sample * gain_right;
            }
            output.push(left);
            output.push(right);
        }
    }
}

/// The channel roles of a negotiated stream, in stream order.
///
/// GStreamer reports positions for every layout it knows. A stream with none —
/// an unpositioned multi-channel file — is read positionally instead: one
/// channel is mono, and more than one keeps the first two as the front pair
/// with the rest folded to silence.
pub(crate) fn roles_from_positions(
    positions: Option<&[AudioChannelPosition]>,
    channels: u16,
) -> Vec<ChannelRole> {
    if let Some(positions) = positions.filter(|found| found.len() == usize::from(channels)) {
        return positions
            .iter()
            .copied()
            .map(ChannelRole::from_position)
            .collect();
    }
    match channels {
        0 => Vec::new(),
        1 => vec![ChannelRole::Mono],
        _ => {
            let mut roles = vec![ChannelRole::Other; usize::from(channels)];
            roles[0] = ChannelRole::FrontLeft;
            roles[1] = ChannelRole::FrontRight;
            roles
        }
    }
}

/// Classifies a negotiated stream's layout from its channel roles.
pub(crate) fn layout_from_roles(roles: &[ChannelRole]) -> ChannelLayout {
    ChannelLayout::from_roles(roles)
}

/// The audio frame a nanosecond timestamp falls on, at `rate`.
///
/// Rounds to the nearest frame: a container stores a start time in
/// nanoseconds, which is not generally a whole number of audio frames, and the
/// nearest frame is the only sample-accurate reading of it.
pub(crate) fn frames_at(nanoseconds: u64, rate: Rational) -> i64 {
    let time = RationalTime::new(
        i64::try_from(nanoseconds).unwrap_or(i64::MAX),
        crate::probe::NANOSECONDS,
    );
    time.rescaled_to_rounding(rate, Rounding::Nearest).value()
}

#[cfg(test)]
mod tests {
    use super::{
        AudioBlock, AudioChannels, AudioFormat, CENTER_DOWNMIX_GAIN, ChannelLayout, ChannelRole,
        LFE_DOWNMIX_GAIN, SURROUND_DOWNMIX_GAIN, StereoDownmix, frames_at, layout_from_roles,
        roles_from_positions,
    };
    use gstreamer_audio::AudioChannelPosition;
    use sub_time::{Rational, RationalTime};

    /// The 5.1 order GStreamer reports: front pair, centre, LFE, surrounds.
    const SURROUND_5_1: [AudioChannelPosition; 6] = [
        AudioChannelPosition::FrontLeft,
        AudioChannelPosition::FrontRight,
        AudioChannelPosition::FrontCenter,
        AudioChannelPosition::Lfe1,
        AudioChannelPosition::RearLeft,
        AudioChannelPosition::RearRight,
    ];

    #[test]
    fn mono_folds_to_both_outputs_at_unity() {
        let roles = roles_from_positions(Some(&[AudioChannelPosition::Mono]), 1);
        assert_eq!(layout_from_roles(&roles), ChannelLayout::Mono);
        let mut out = Vec::new();
        StereoDownmix::from_roles(&roles).apply(&[0.5, -0.25], &mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.25, -0.25]);
    }

    #[test]
    fn stereo_passes_through_untouched() {
        let roles = roles_from_positions(
            Some(&[
                AudioChannelPosition::FrontLeft,
                AudioChannelPosition::FrontRight,
            ]),
            2,
        );
        assert_eq!(layout_from_roles(&roles), ChannelLayout::Stereo);
        let mut out = Vec::new();
        StereoDownmix::from_roles(&roles).apply(&[0.25, -0.5, 1.0, 0.0], &mut out);
        assert_eq!(out, vec![0.25, -0.5, 1.0, 0.0]);
    }

    #[test]
    fn five_one_folds_with_the_documented_gains() {
        let roles = roles_from_positions(Some(&SURROUND_5_1), 6);
        assert_eq!(layout_from_roles(&roles), ChannelLayout::Surround5_1);
        let downmix = StereoDownmix::from_roles(&roles);
        assert_eq!(
            downmix.gains(),
            [
                (1.0, 0.0),
                (0.0, 1.0),
                (CENTER_DOWNMIX_GAIN, CENTER_DOWNMIX_GAIN),
                (LFE_DOWNMIX_GAIN, LFE_DOWNMIX_GAIN),
                (SURROUND_DOWNMIX_GAIN, 0.0),
                (0.0, SURROUND_DOWNMIX_GAIN),
            ]
        );

        // One frame: L, R, C, LFE, Ls, Rs.
        let mut out = Vec::new();
        downmix.apply(&[1.0, 2.0, 4.0, 8.0, 16.0, 32.0], &mut out);
        assert_eq!(out.len(), 2);
        let left = 4.0_f32.mul_add(CENTER_DOWNMIX_GAIN, 16.0 * SURROUND_DOWNMIX_GAIN) + 1.0;
        let right = 4.0_f32.mul_add(CENTER_DOWNMIX_GAIN, 32.0 * SURROUND_DOWNMIX_GAIN) + 2.0;
        assert!((out[0] - left).abs() < 1e-5, "left {} vs {left}", out[0]);
        assert!((out[1] - right).abs() < 1e-5, "right {} vs {right}", out[1]);

        // The LFE contributes nothing at all.
        let mut lfe_only = Vec::new();
        downmix.apply(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut lfe_only);
        assert_eq!(lfe_only, vec![0.0, 0.0]);
    }

    #[test]
    fn side_channels_fold_like_rear_channels() {
        let roles = roles_from_positions(
            Some(&[
                AudioChannelPosition::FrontLeft,
                AudioChannelPosition::FrontRight,
                AudioChannelPosition::FrontCenter,
                AudioChannelPosition::Lfe1,
                AudioChannelPosition::SideLeft,
                AudioChannelPosition::SideRight,
            ]),
            6,
        );
        assert_eq!(layout_from_roles(&roles), ChannelLayout::Surround5_1);
        assert_eq!(roles[4], ChannelRole::SurroundLeft);
        assert_eq!(roles[5], ChannelRole::SurroundRight);
    }

    #[test]
    fn an_unpositioned_stream_keeps_its_first_two_channels() {
        let roles = roles_from_positions(None, 4);
        assert_eq!(layout_from_roles(&roles), ChannelLayout::Other);
        let mut out = Vec::new();
        StereoDownmix::from_roles(&roles).apply(&[1.0, 2.0, 3.0, 4.0], &mut out);
        assert_eq!(out, vec![1.0, 2.0]);
    }

    #[test]
    fn an_unpositioned_mono_stream_is_still_mono() {
        assert_eq!(
            layout_from_roles(&roles_from_positions(None, 1)),
            ChannelLayout::Mono
        );
    }

    #[test]
    fn positions_that_do_not_match_the_channel_count_are_ignored() {
        let roles = roles_from_positions(Some(&SURROUND_5_1), 2);
        assert_eq!(layout_from_roles(&roles), ChannelLayout::Stereo);
    }

    #[test]
    fn timestamps_land_on_whole_audio_frames() {
        let rate = Rational::HZ_48000;
        assert_eq!(frames_at(0, rate), 0);
        assert_eq!(frames_at(1_000_000_000, rate), 48_000);
        // 1/30 s is exactly 1600 frames at 48 kHz.
        assert_eq!(frames_at(1_000_000_000 / 30, rate), 1_600);
        // A timestamp between two frames rounds to the nearer one.
        assert_eq!(frames_at(20_834, rate), 1);
    }

    #[test]
    fn a_block_reports_its_frames_and_its_end() {
        let rate = Rational::HZ_48000;
        let samples = [0.0_f32; 8];
        let block = AudioBlock {
            start: RationalTime::new(48_000, rate),
            sample_rate: 48_000,
            channels: 2,
            samples: &samples,
        };
        assert_eq!(block.frames(), 4);
        assert_eq!(block.end(), RationalTime::new(48_004, rate));
    }

    #[test]
    fn a_format_reports_its_frame_rate_and_whether_it_folds() {
        let format = AudioFormat {
            sample_rate: 44_100,
            source_channels: 6,
            source_layout: ChannelLayout::Surround5_1,
            channels: 2,
        };
        assert_eq!(format.frame_rate(), Rational::new(44_100, 1).expect("rate"));
        assert!(format.is_downmixed());
        assert_eq!(format.source_layout.as_str(), "5.1");
        assert_eq!(AudioChannels::default(), AudioChannels::StereoDownmix);
    }
}
