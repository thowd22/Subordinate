//! The A/V sync and drift harness: does the picture stay with the sound over
//! the whole length of the long fixture?
//!
//! Sync is defined by audio (docs/PLAN.md §5.4). The output callback publishes
//! an [`sub_audio::AudioClock`], `PlaybackScheduler` picks the frame that
//! covers it, and the viewer shows whatever the decoder has reached by then.
//! Phase 3's exit criterion is that this relationship holds for ten minutes,
//! so it is measured rather than argued about.
//!
//! One run drives the production parts end to end, headlessly and as fast as
//! the machine will go:
//!
//! * `sub_audio::OutputRenderer` renders real callback-sized blocks from the
//!   mixer at the sequence rate into a device running at a *different* rate,
//!   so the sample-rate conversion is exercised, and publishes the clock;
//! * `sub_edit::playback::PlaybackScheduler` follows that clock and picks the
//!   frame to show;
//! * `sub_media::Decoder` decodes the long fixture forward to that frame,
//!   exactly as the viewer's decode-ahead does; frames the master ran past
//!   are dropped rather than shown late.
//!
//! Once per second of timeline the audible position is sampled against the
//! PTS of the frame on screen. The arithmetic is integer throughout: drift is
//! counted in milli-frames (`1_000` is one whole frame), never in floats.
//!
//! The long fixture carries no audio track, so the master here is the
//! sequence transport running silence over the fixture's timeline. That is
//! the clock relationship under test; what the samples contain does not enter
//! into it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sub_audio::mixer::{MixGraphBuilder, MixerConfig, mixer};
use sub_audio::output::{
    OutputDeviceInfo, OutputMetrics, OutputRenderer, OutputSampleFormat, SupportedFormat,
};
use sub_core::SubResult;
use sub_edit::playback::PlaybackScheduler;
use sub_media::{Decoder, DecoderOptions, FrameFormat, HardwarePreference, VideoFrame};
use sub_time::{Rational, RationalTime, Rounding};

use crate::report::SyncSection;

/// The fixture the sync harness plays: ten minutes of long-GOP 720p.
pub const FIXTURE: &str = "longgop_720p_10min.mp4";

/// The sequence sample rate the mixer renders at.
pub const SEQUENCE_RATE: u32 = 48_000;

/// The device rate, deliberately not the sequence rate so every block goes
/// through the sample-rate conversion the real output path would do.
pub const DEVICE_RATE: u32 = 44_100;

/// Device frames per callback: a common low-latency buffer.
pub const BLOCK_FRAMES: usize = 512;

/// Output channels, for both the sequence and the device.
const CHANNELS: u16 = 2;

/// One whole frame, in the milli-frame units drift is counted in.
pub const FRAME_MILLI: i64 = 1_000;

/// How often the audible position is sampled against the frame on screen.
pub const SAMPLE_INTERVAL_SECONDS: i64 = 1;

/// What one sync run should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Where the fixtures live; `None` means the usual lookup, which honours
    /// `SUB_FIXTURES_DIR`.
    pub fixtures_dir: Option<PathBuf>,
    /// Whether a hardware decoder may be preferred.
    pub hardware: HardwarePreference,
    /// Stop after this many seconds of timeline; `None` plays the whole
    /// fixture, which is what the exit criterion is stated against.
    pub seconds: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fixtures_dir: None,
            hardware: HardwarePreference::Prefer,
            seconds: None,
        }
    }
}

/// Plays the long fixture and measures how far the picture ever gets from the
/// sound.
///
/// # Errors
///
/// Only genuine failures of the paths under test: a decoder that errors, a
/// mixer or renderer that will not build. A machine without the fixture gets
/// a skipped section instead.
pub fn run(options: &Options) -> SubResult<SyncSection> {
    let dir = options
        .fixtures_dir
        .clone()
        .unwrap_or_else(sub_test_support::fixtures_dir);
    let (path, rate, duration) = match locate(&dir) {
        Ok(found) => found,
        Err(reason) => {
            tracing::warn!(fixture = FIXTURE, %reason, "skipping the A/V sync run");
            return Ok(SyncSection::skipped(FIXTURE, &reason));
        }
    };
    let seconds = options
        .seconds
        .map_or(duration, |limit| limit.min(duration));
    measure(&path, rate, seconds, options)
}

/// The fixture's path, frame rate and length in whole seconds, or why the run
/// cannot happen here.
fn locate(dir: &Path) -> Result<(PathBuf, Rational, u64), String> {
    let path = sub_test_support::fixture_from(dir, FIXTURE).map_err(|error| error.to_string())?;
    let manifest = sub_test_support::load_manifest_from(dir).map_err(|error| error.to_string())?;
    let fixture = manifest
        .get(FIXTURE)
        .ok_or_else(|| format!("no fixture named '{FIXTURE}' in the manifest"))?;
    let (num, den) = fixture.fps();
    let rate = Rational::new(num, den)
        .ok_or_else(|| format!("{FIXTURE} declares no usable frame rate"))?;
    let seconds = fixture.duration_ns / 1_000_000_000;
    if seconds == 0 {
        return Err(format!("{FIXTURE} is shorter than a second"));
    }
    Ok((path, rate, seconds))
}

/// One measured run over `seconds` of the fixture's timeline.
fn measure(
    path: &Path,
    video_rate: Rational,
    seconds: u64,
    options: &Options,
) -> SubResult<SyncSection> {
    let audio_rate = Rational::from_integer(SEQUENCE_RATE).expect("48 kHz is a valid timebase");
    let mut audio = AudioMaster::new()?;
    let mut scheduler = PlaybackScheduler::new(video_rate);
    scheduler.set_duration(
        RationalTime::new(
            i64::try_from(seconds).unwrap_or(i64::MAX) * i64::from(SEQUENCE_RATE),
            audio_rate,
        )
        .rescaled_to_rounding(video_rate, Rounding::Ceil),
    );
    scheduler.play_forward();

    let mut decoder = Decoder::open_with(
        path,
        DecoderOptions {
            hardware: options.hardware,
            format: FrameFormat::Nv12,
            ..DecoderOptions::default()
        },
    )?;
    let mut screen = Screen::new(&mut decoder)?;

    let mut section = SyncSection::measured(FIXTURE, video_rate, seconds);
    let mut worst = Drift::default();
    let mut next_sample = RationalTime::zero(audio_rate);
    let sample_step = i64::from(SEQUENCE_RATE) * SAMPLE_INTERVAL_SECONDS;
    let blocks = u64::from(DEVICE_RATE) * seconds / BLOCK_FRAMES as u64;

    for _ in 0..blocks {
        let master = audio.render_block();
        scheduler.follow(master);
        screen.present(&mut decoder, scheduler.position())?;
        if master.value() >= next_sample.value() {
            if let Some(pts) = screen.pts {
                let drift = Drift::measure(master, pts, video_rate);
                worst = worst.worse_of(drift);
                section.samples += 1;
            }
            next_sample = RationalTime::new(next_sample.value() + sample_step, audio_rate);
        }
        if !scheduler.is_playing() {
            break;
        }
    }

    section.seconds_played = audio.clock.position_frames().unwrap_or(0) / u64::from(SEQUENCE_RATE);
    section.decoder = decoder.decoder_element();
    section.frames_shown = screen.shown;
    section.frames_decoded = screen.decoded;
    section.dropped_frames = scheduler.dropped_frames();
    section.max_drift_milli_frames = worst.milli_frames;
    section.max_offset_milli_frames = worst.offset_milli_frames;
    section.worst_at_seconds = worst.at_seconds;
    Ok(section)
}

/// The audio side of the run: a mixer feeding a resampling output renderer,
/// stepped one callback block at a time.
struct AudioMaster {
    /// The renderer whose callback publishes the clock.
    renderer: OutputRenderer,
    /// The clock that callback publishes into.
    clock: Arc<sub_audio::AudioClock>,
    /// The interleaved device buffer one block is rendered into.
    block: Vec<f32>,
    /// Kept alive so the mixer keeps its graph for the whole run.
    _control: sub_audio::MixerControl,
}

impl AudioMaster {
    /// Builds the audio path: a silent sequence at [`SEQUENCE_RATE`] rendered
    /// into a device that only offers [`DEVICE_RATE`].
    fn new() -> SubResult<Self> {
        let graph = MixGraphBuilder::new(SEQUENCE_RATE, CHANNELS).build()?;
        let (control, mixer) = mixer(graph, MixerConfig::default())?;
        let device = OutputDeviceInfo::new(
            "av-sync",
            "A/V sync harness device",
            vec![SupportedFormat::new(
                CHANNELS,
                DEVICE_RATE,
                DEVICE_RATE,
                OutputSampleFormat::F32,
            )],
        );
        let format = device.negotiate(SEQUENCE_RATE, CHANNELS)?;
        let renderer =
            OutputRenderer::new(mixer, format, Arc::new(OutputMetrics::new()), BLOCK_FRAMES)?;
        let clock = Arc::clone(renderer.clock());
        Ok(Self {
            renderer,
            clock,
            block: vec![0.0; BLOCK_FRAMES * usize::from(CHANNELS)],
            _control: control,
        })
    }

    /// Renders one callback block and returns the audible position after it.
    fn render_block(&mut self) -> RationalTime {
        self.renderer.render(&mut self.block);
        self.clock
            .position()
            .expect("the callback published a position")
    }
}

/// What is on screen: the frame the decoder has reached for the playhead the
/// scheduler chose.
struct Screen {
    /// The next decoded frame, not yet due to be shown.
    pending: Option<VideoFrame>,
    /// The PTS of the frame currently on screen.
    pts: Option<RationalTime>,
    /// How many frames were put on screen.
    shown: u64,
    /// How many frames came out of the decoder.
    decoded: u64,
}

impl Screen {
    /// Primes the screen with the first decoded frame pending.
    fn new(decoder: &mut Decoder) -> SubResult<Self> {
        let first = decoder.next_frame()?;
        Ok(Self {
            decoded: u64::from(first.is_some()),
            pending: first,
            pts: None,
            shown: 0,
        })
    }

    /// Decodes forward until the frame covering `playhead` is on screen.
    ///
    /// Frames the playhead has already run past are dropped rather than shown
    /// late, which is what the viewer does when decode falls behind.
    fn present(&mut self, decoder: &mut Decoder, playhead: RationalTime) -> SubResult<()> {
        while self
            .pending
            .as_ref()
            .is_some_and(|frame| !is_after(frame.pts(), playhead))
        {
            let frame = self.pending.take().expect("the frame was just inspected");
            self.pts = Some(frame.pts());
            self.shown += 1;
            self.pending = decoder.next_frame()?;
            self.decoded += u64::from(self.pending.is_some());
        }
        Ok(())
    }
}

/// True when `time` is strictly later than `other`, comparing exactly across
/// their timebases.
fn is_after(time: RationalTime, other: RationalTime) -> bool {
    let (left_num, left_den) = time.as_seconds_fraction();
    let (right_num, right_den) = other.as_seconds_fraction();
    left_num * right_den > right_num * left_den
}

/// How far the picture is from the sound at one sample point.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Drift {
    /// Drift in milli-frames: zero while the audible position lies inside
    /// the displayed frame's own interval, otherwise the signed distance
    /// beyond it. Positive means the picture is behind the sound.
    pub milli_frames: i64,
    /// The raw offset in milli-frames: the audible position minus the PTS of
    /// the frame on screen. A frame is shown for a whole frame interval, so
    /// this sits in `0..1_000` while sync holds.
    pub offset_milli_frames: i64,
    /// Whole seconds into the run at which this sample was taken.
    pub at_seconds: u64,
}

impl Drift {
    /// Samples the audible position `master` against the PTS of the frame on
    /// screen, at the video timebase `rate`.
    pub fn measure(master: RationalTime, pts: RationalTime, rate: Rational) -> Self {
        let offset = offset_milli_frames(master, pts, rate);
        Self {
            milli_frames: beyond_the_frame(offset),
            offset_milli_frames: offset,
            at_seconds: seconds_of(master),
        }
    }

    /// Whichever of two samples drifted further, keeping where it happened.
    pub fn worse_of(self, other: Self) -> Self {
        if other.milli_frames.abs() > self.milli_frames.abs()
            || (other.milli_frames.abs() == self.milli_frames.abs()
                && other.offset_milli_frames.abs() > self.offset_milli_frames.abs())
        {
            other
        } else {
            self
        }
    }
}

/// The audible position minus the displayed frame's PTS, in milli-frames.
///
/// Exact integer arithmetic over both timebases: the times are compared as
/// rational seconds and scaled by the frame rate, so 23.976 and 25 are both
/// exact and nothing is ever a float. Truncated towards zero.
pub fn offset_milli_frames(master: RationalTime, pts: RationalTime, rate: Rational) -> i64 {
    let (master_num, master_den) = master.as_seconds_fraction();
    let (pts_num, pts_den) = pts.as_seconds_fraction();
    let numerator = (master_num * pts_den - pts_num * master_den)
        * i128::from(rate.numerator())
        * i128::from(FRAME_MILLI);
    let denominator = master_den * pts_den * i128::from(rate.denominator());
    if denominator == 0 {
        return 0;
    }
    i64::try_from(numerator / denominator).unwrap_or(i64::MAX)
}

/// How far `offset` reaches outside the displayed frame's own interval.
///
/// A frame is on screen for a whole frame interval, so any offset in
/// `0..1_000` milli-frames is the right frame being shown at the right time
/// and reads as no drift. Anything else is the signed distance beyond that
/// interval: negative when the picture has run ahead of the sound, positive
/// when it lags behind.
pub const fn beyond_the_frame(offset: i64) -> i64 {
    if offset < 0 {
        offset
    } else if offset >= FRAME_MILLI {
        offset - FRAME_MILLI + 1
    } else {
        0
    }
}

/// Whole seconds a position is into the timeline.
fn seconds_of(time: RationalTime) -> u64 {
    let (num, den) = time.as_seconds_fraction();
    if den <= 0 || num <= 0 {
        return 0;
    }
    u64::try_from(num / den).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        BLOCK_FRAMES, DEVICE_RATE, Drift, FRAME_MILLI, Options, SEQUENCE_RATE, beyond_the_frame,
        is_after, offset_milli_frames, seconds_of,
    };
    use sub_media::HardwarePreference;
    use sub_media::probe::NANOSECONDS;
    use sub_time::{Rational, RationalTime};

    fn audio(samples: i64) -> RationalTime {
        RationalTime::new(
            samples,
            Rational::from_integer(SEQUENCE_RATE).expect("a rate"),
        )
    }

    fn nanos(value: i64) -> RationalTime {
        RationalTime::new(value, NANOSECONDS)
    }

    #[test]
    fn the_defaults_play_the_whole_fixture_with_hardware_allowed() {
        let options = Options::default();
        assert_eq!(options.seconds, None);
        assert_eq!(options.hardware, HardwarePreference::Prefer);
        assert!(options.fixtures_dir.is_none());
    }

    #[test]
    fn the_device_rate_differs_from_the_sequence_rate() {
        // The point of the harness is that the resampler runs.
        assert_ne!(SEQUENCE_RATE, DEVICE_RATE);
        const { assert!(BLOCK_FRAMES > 0) };
    }

    #[test]
    fn an_offset_of_exactly_one_frame_is_one_thousand_milli_frames() {
        let rate = Rational::new(25, 1).expect("25 fps");
        // 40 ms at 25 fps is one frame; 48 kHz counts it as 1920 samples.
        assert_eq!(offset_milli_frames(audio(1_920), nanos(0), rate), 1_000);
        assert_eq!(offset_milli_frames(audio(960), nanos(0), rate), 500);
        assert_eq!(
            offset_milli_frames(audio(0), nanos(40_000_000), rate),
            -1_000
        );
    }

    #[test]
    fn a_fractional_frame_rate_stays_exact() {
        let rate = Rational::new(24_000, 1_001).expect("23.976 fps");
        // One frame is 1001/24000 s, which is 2002 samples at 48 kHz.
        assert_eq!(offset_milli_frames(audio(2_002), nanos(0), rate), 1_000);
        assert_eq!(offset_milli_frames(audio(1_001), nanos(0), rate), 500);
    }

    #[test]
    fn the_frame_on_screen_for_its_own_interval_has_not_drifted() {
        for offset in [0, 1, 499, 999] {
            assert_eq!(beyond_the_frame(offset), 0, "offset {offset}");
        }
        // A whole frame late is one milli-frame beyond the interval, not
        // suddenly a thousand.
        assert_eq!(beyond_the_frame(1_000), 1);
        assert_eq!(beyond_the_frame(1_999), 1_000);
        assert_eq!(beyond_the_frame(-1), -1);
        assert_eq!(beyond_the_frame(-1_000), -1_000);
    }

    #[test]
    fn a_drift_sample_records_where_it_happened() {
        let rate = Rational::new(25, 1).expect("25 fps");
        // Five seconds in, showing the frame that started 100 ms earlier:
        // two and a half frames behind.
        let drift = Drift::measure(audio(5 * 48_000), nanos(4_900_000_000), rate);
        assert_eq!(drift.offset_milli_frames, 2_500);
        assert_eq!(drift.milli_frames, 2_500 - FRAME_MILLI + 1);
        assert_eq!(drift.at_seconds, 5);
    }

    #[test]
    fn the_worse_sample_wins_whichever_side_it_is_on() {
        let rate = Rational::new(25, 1).expect("25 fps");
        let inside = Drift::measure(audio(960), nanos(0), rate);
        let ahead = Drift::measure(audio(0), nanos(120_000_000), rate);
        assert_eq!(inside.milli_frames, 0);
        assert_eq!(ahead.milli_frames, -3_000);
        assert_eq!(inside.worse_of(ahead), ahead);
        assert_eq!(ahead.worse_of(inside), ahead);
    }

    #[test]
    fn times_compare_exactly_across_timebases() {
        assert!(is_after(nanos(40_000_001), audio(1_920)));
        assert!(!is_after(nanos(40_000_000), audio(1_920)));
        assert!(!is_after(nanos(0), audio(1_920)));
    }

    #[test]
    fn seconds_are_counted_whole_and_never_negative() {
        assert_eq!(seconds_of(audio(0)), 0);
        assert_eq!(seconds_of(audio(47_999)), 0);
        assert_eq!(seconds_of(audio(48_000)), 1);
        assert_eq!(seconds_of(audio(-48_000)), 0);
    }
}
