//! The cpal output stage: device listing, format negotiation and the
//! real-time playback callback (docs/PLAN.md §5.4).
//!
//! The mixer renders at the *sequence* rate and channel count. A device does
//! not have to agree: this module negotiates the closest shape the device
//! advertises, then adapts the mixer's blocks to it inside the callback with
//! an [`OutputRenderer`]. The renderer allocates nothing, locks nothing and
//! makes no syscall — every buffer it touches is sized when the stream opens,
//! and the sample-rate conversion is driven by an exact integer accumulator
//! rather than a floating-point phase, so it never drifts.
//!
//! The host side is behind [`OutputBackend`] so device listing, opening and
//! switching can be exercised without audio hardware; [`CpalBackend`] is the
//! real one. [`AudioOutput`] owns the current stream and the user's device
//! choice, and reopens the stream when that choice changes — falling back to
//! the device that was playing when the new one cannot be opened.
//!
//! ```
//! # fn main() -> sub_core::SubResult<()> {
//! use sub_audio::mixer::{MixGraphBuilder, MixerConfig, TrackSpec, mixer};
//! use sub_audio::output::{OutputSampleFormat, SupportedFormat, negotiate};
//!
//! // A device that only does 44.1 kHz stereo in 16-bit.
//! let supported = [SupportedFormat::new(
//!     2,
//!     44_100,
//!     44_100,
//!     OutputSampleFormat::I16,
//! )];
//! let format = negotiate(&supported, 48_000, 2)?;
//! assert_eq!(format.sample_rate(), 44_100);
//! assert!(format.needs_resampling());
//! # Ok(())
//! # }
//! ```

use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use sub_core::{SubError, SubResult};

use crate::codes;
use crate::decode::MAX_CHANNELS;
use crate::mixer::Mixer;

/// A sample format the output stage can write into a device buffer.
///
/// Everything upstream is `f32`; these are the shapes a host may ask for on
/// the way out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutputSampleFormat {
    /// 32-bit float in `-1.0..=1.0`, written straight through.
    F32,
    /// 16-bit signed, zero-centred.
    I16,
    /// 16-bit unsigned, centred on `1 << 15`.
    U16,
}

impl OutputSampleFormat {
    /// How strongly this stage prefers the format: lower is better.
    ///
    /// `f32` is a straight copy; the integer formats cost a conversion and a
    /// quantisation, and unsigned costs an extra bias on top.
    fn rank(self) -> u8 {
        match self {
            Self::F32 => 0,
            Self::I16 => 1,
            Self::U16 => 2,
        }
    }

    /// The format's stable name, as diagnostics print it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::I16 => "i16",
            Self::U16 => "u16",
        }
    }
}

impl fmt::Display for OutputSampleFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One stream shape a device advertises: a channel count, a sample format and
/// the range of sample rates the pair covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportedFormat {
    /// Channels per frame.
    channels: u16,
    /// The lowest rate in the range, in hertz.
    min_sample_rate: u32,
    /// The highest rate in the range, in hertz.
    max_sample_rate: u32,
    /// The sample type the device wants.
    sample_format: OutputSampleFormat,
}

impl SupportedFormat {
    /// Describes a supported shape. The rate bounds are ordered here, so
    /// either argument order is accepted.
    pub fn new(
        channels: u16,
        min_sample_rate: u32,
        max_sample_rate: u32,
        sample_format: OutputSampleFormat,
    ) -> Self {
        Self {
            channels,
            min_sample_rate: min_sample_rate.min(max_sample_rate),
            max_sample_rate: min_sample_rate.max(max_sample_rate),
            sample_format,
        }
    }

    /// Channels per frame.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// The lowest rate the shape covers, in hertz.
    pub fn min_sample_rate(&self) -> u32 {
        self.min_sample_rate
    }

    /// The highest rate the shape covers, in hertz.
    pub fn max_sample_rate(&self) -> u32 {
        self.max_sample_rate
    }

    /// The sample type the device wants.
    pub fn sample_format(&self) -> OutputSampleFormat {
        self.sample_format
    }

    /// Whether `rate` falls inside the shape's range.
    pub fn contains_rate(&self, rate: u32) -> bool {
        (self.min_sample_rate..=self.max_sample_rate).contains(&rate)
    }

    /// The rate in this shape's range closest to `rate`.
    pub fn nearest_rate(&self, rate: u32) -> u32 {
        rate.clamp(self.min_sample_rate, self.max_sample_rate)
    }
}

/// A device the host offers for playback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDeviceInfo {
    /// The host's stable identifier, or the name when the host has none.
    /// This is what a project or a settings file stores.
    id: String,
    /// What the device calls itself, for the settings list.
    name: String,
    /// Whether the host would pick this device by default.
    is_default: bool,
    /// The shapes it advertises, in the order the host reported them.
    formats: Vec<SupportedFormat>,
    /// The rate the host would open it at, when it says.
    default_sample_rate: Option<u32>,
}

impl OutputDeviceInfo {
    /// Describes a device.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        formats: Vec<SupportedFormat>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            is_default: false,
            formats,
            default_sample_rate: None,
        }
    }

    /// Marks the device as the host's default.
    #[must_use]
    pub fn with_default(mut self, is_default: bool) -> Self {
        self.is_default = is_default;
        self
    }

    /// Records the rate the host would open the device at.
    #[must_use]
    pub fn with_default_sample_rate(mut self, rate: Option<u32>) -> Self {
        self.default_sample_rate = rate;
        self
    }

    /// The host's stable identifier for the device.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What the device calls itself.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the host would pick this device by default.
    pub fn is_default(&self) -> bool {
        self.is_default
    }

    /// The shapes the device advertises.
    pub fn formats(&self) -> &[SupportedFormat] {
        &self.formats
    }

    /// The rate the host would open the device at, when it says.
    pub fn default_sample_rate(&self) -> Option<u32> {
        self.default_sample_rate
    }

    /// Whether some advertised shape covers `rate` exactly.
    pub fn supports_rate(&self, rate: u32) -> bool {
        self.formats.iter().any(|f| f.contains_rate(rate))
    }

    /// Picks the shape closest to a sequence at `sample_rate` and `channels`.
    ///
    /// # Errors
    ///
    /// See [`negotiate`].
    pub fn negotiate(&self, sample_rate: u32, channels: u16) -> SubResult<NegotiatedFormat> {
        negotiate(&self.formats, sample_rate, channels)
    }
}

/// The shape a stream actually runs at, and the sequence shape it is fed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiatedFormat {
    /// The device's rate, in hertz.
    sample_rate: u32,
    /// The device's channels per frame.
    channels: u16,
    /// The sample type the device wants.
    sample_format: OutputSampleFormat,
    /// The sequence rate the mixer renders at, in hertz.
    source_sample_rate: u32,
    /// The sequence channels per frame the mixer renders.
    source_channels: u16,
}

impl NegotiatedFormat {
    /// The device's rate, in hertz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The device's channels per frame.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// The sample type the device wants.
    pub fn sample_format(&self) -> OutputSampleFormat {
        self.sample_format
    }

    /// The sequence rate the mixer renders at, in hertz.
    pub fn source_sample_rate(&self) -> u32 {
        self.source_sample_rate
    }

    /// The sequence channels per frame the mixer renders.
    pub fn source_channels(&self) -> u16 {
        self.source_channels
    }

    /// Whether the callback has to convert the mixer's rate to the device's.
    pub fn needs_resampling(&self) -> bool {
        self.sample_rate != self.source_sample_rate
    }

    /// Whether the callback has to remap the mixer's channels to the device's.
    pub fn needs_channel_map(&self) -> bool {
        self.channels != self.source_channels
    }
}

impl fmt::Display for NegotiatedFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} Hz, {} ch, {}",
            self.sample_rate, self.channels, self.sample_format
        )?;
        if self.needs_resampling() {
            write!(f, " (resampled from {} Hz)", self.source_sample_rate)?;
        }
        if self.needs_channel_map() {
            write!(f, " (mapped from {} ch)", self.source_channels)?;
        }
        Ok(())
    }
}

/// Picks the shape in `formats` closest to a sequence at `sample_rate` and
/// `channels`.
///
/// The sequence rate wins whenever a shape covers it, so a project at 48 kHz
/// on a device that does 48 kHz plays with no conversion at all. Otherwise the
/// closest rate any shape can reach is taken and the callback resamples.
/// Between shapes that are equally close on rate, the one whose channel count
/// matches the sequence wins, then the one with more channels (which the
/// callback can leave silent) over one with fewer (which has to be folded
/// down), then `f32` over the integer formats.
///
/// # Errors
///
/// Returns [`sub_core::codes::INVALID_ARGUMENT`] when `sample_rate` is zero or
/// `channels` is zero or above [`MAX_CHANNELS`], and
/// [`codes::FORMAT_UNSUPPORTED`] when `formats` is empty.
pub fn negotiate(
    formats: &[SupportedFormat],
    sample_rate: u32,
    channels: u16,
) -> SubResult<NegotiatedFormat> {
    if sample_rate == 0 {
        return Err(SubError::new(
            sub_core::codes::INVALID_ARGUMENT,
            "a sequence sample rate must be greater than zero",
        ));
    }
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(SubError::new(
            sub_core::codes::INVALID_ARGUMENT,
            "a sequence must have between one and MAX_CHANNELS channels",
        )
        .with_detail("channels", channels)
        .with_detail("max_channels", MAX_CHANNELS));
    }
    let best = formats
        .iter()
        .filter(|format| format.channels > 0)
        .min_by_key(|format| {
            let rate = format.nearest_rate(sample_rate);
            let rate_distance = rate.abs_diff(sample_rate);
            // Exact channel match first, then a wider device (extra channels
            // go silent) ahead of a narrower one (channels must be folded).
            let channel_rank = match format.channels.cmp(&channels) {
                std::cmp::Ordering::Equal => 0u8,
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Less => 2,
            };
            let channel_distance = format.channels.abs_diff(channels);
            (
                rate_distance,
                channel_rank,
                channel_distance,
                format.sample_format.rank(),
                rate,
                format.channels,
            )
        })
        .ok_or_else(|| {
            SubError::new(
                codes::FORMAT_UNSUPPORTED,
                "the device advertises no output format this build can write",
            )
            .with_detail("sample_rate", sample_rate)
            .with_detail("channels", channels)
        })?;
    Ok(NegotiatedFormat {
        sample_rate: best.nearest_rate(sample_rate),
        channels: best.channels,
        sample_format: best.sample_format,
        source_sample_rate: sample_rate,
        source_channels: channels,
    })
}

/// Counters the output stage keeps, read from any thread.
///
/// The audio callback only ever stores into the atomics — one relaxed store
/// per field per block — so reading these never disturbs playback.
/// `last_error` is a lock, but only the host's error callback and the reader
/// ever touch it; the audio callback does not.
#[derive(Debug, Default)]
pub struct OutputMetrics {
    /// Frames a clip owed the mixer but its ring had not produced, cumulative
    /// for the life of this stream.
    underrun_frames: AtomicU64,
    /// Device frames handed to the host since the stream opened.
    frames_rendered: AtomicU64,
    /// Callbacks the host has made since the stream opened.
    callbacks: AtomicU64,
    /// Errors the host reported on the stream.
    stream_errors: AtomicU64,
    /// The most recent host error, for the diagnostics panel.
    last_error: Mutex<Option<String>>,
}

impl OutputMetrics {
    /// Fresh counters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames the mixer could not fill because a clip ring had run dry.
    pub fn underrun_frames(&self) -> u64 {
        self.underrun_frames.load(Ordering::Relaxed)
    }

    /// Device frames handed to the host since the stream opened.
    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered.load(Ordering::Relaxed)
    }

    /// Callbacks the host has made since the stream opened.
    pub fn callbacks(&self) -> u64 {
        self.callbacks.load(Ordering::Relaxed)
    }

    /// Errors the host reported on the stream.
    pub fn stream_errors(&self) -> u64 {
        self.stream_errors.load(Ordering::Relaxed)
    }

    /// The most recent host error, when there has been one.
    pub fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Records a host error. Called from the host's error callback, which is
    /// not the audio thread.
    pub fn record_stream_error(&self, message: impl Into<String>) {
        let message = message.into();
        tracing::warn!(target: "sub_audio::output", error = %message, "audio output stream error");
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
        *self
            .last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(message);
    }

    /// Publishes what one callback did. Real-time safe: three relaxed stores.
    fn record_block(&self, frames: u64, underrun_frames: u64) {
        self.frames_rendered.fetch_add(frames, Ordering::Relaxed);
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.underrun_frames
            .store(underrun_frames, Ordering::Relaxed);
    }
}

/// A snapshot of what the output stage is doing, for the diagnostics panel and
/// `subordinate-cli`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDiagnostics {
    /// What the device calls itself.
    pub device_name: String,
    /// The host's identifier for the device.
    pub device_id: String,
    /// The shape the stream runs at.
    pub format: NegotiatedFormat,
    /// Frames lost to clip rings that had run dry.
    pub underrun_frames: u64,
    /// Device frames handed to the host since the stream opened.
    pub frames_rendered: u64,
    /// Callbacks the host has made since the stream opened.
    pub callbacks: u64,
    /// Errors the host reported on the stream.
    pub stream_errors: u64,
    /// The most recent host error, when there has been one.
    pub last_error: Option<String>,
}

impl OutputDiagnostics {
    /// Whether anything has gone wrong on this stream.
    pub fn is_healthy(&self) -> bool {
        self.underrun_frames == 0 && self.stream_errors == 0
    }
}

impl fmt::Display for OutputDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} — {} frames in {} callbacks, {} underrun frames, {} stream errors",
            self.device_name,
            self.format,
            self.frames_rendered,
            self.callbacks,
            self.underrun_frames,
            self.stream_errors
        )?;
        if let Some(error) = &self.last_error {
            write!(f, " (last error: {error})")?;
        }
        Ok(())
    }
}

/// Maps one block from `src_channels` to `dst_channels`, both interleaved.
///
/// Mono fans out to the first two device channels; a wider device leaves the
/// channels the sequence does not fill silent; a mono device sums the sequence
/// down; a narrower multi-channel device takes the first channels it has room
/// for. `src` and `dst` must hold the same number of frames.
///
/// Real-time safe: arithmetic on two slices, nothing else.
#[allow(
    clippy::cast_precision_loss,
    reason = "a channel count is far below f32's exact integer range"
)]
fn map_channels(src: &[f32], src_channels: usize, dst: &mut [f32], dst_channels: usize) {
    debug_assert!(src_channels > 0 && dst_channels > 0);
    for (frame_in, frame_out) in src
        .chunks_exact(src_channels)
        .zip(dst.chunks_exact_mut(dst_channels))
    {
        if src_channels == 1 {
            for (index, out) in frame_out.iter_mut().enumerate() {
                *out = if index < 2 { frame_in[0] } else { 0.0 };
            }
        } else if dst_channels == 1 {
            let sum: f32 = frame_in.iter().sum();
            frame_out[0] = sum / src_channels as f32;
        } else {
            for (index, out) in frame_out.iter_mut().enumerate() {
                *out = frame_in.get(index).copied().unwrap_or(0.0);
            }
        }
    }
}

/// The audio callback's body: a [`Mixer`], the buffers the conversion needs
/// and the counters it publishes.
///
/// Every buffer is sized in [`OutputRenderer::new`]. `render` and its integer
/// variants allocate nothing, lock nothing and never block, so they are safe
/// to call straight from a host callback.
///
/// The sample-rate conversion is a linear interpolation on an exact rational
/// clock: the position between two source frames is held as `phase / device
/// rate` and advanced by the sequence rate once per device frame, so the
/// stream never accumulates the rounding drift a floating-point phase would.
pub struct OutputRenderer {
    /// The mixer, rendering at the sequence rate.
    mixer: Mixer,
    /// The buffers and the clock the conversion runs on.
    converter: Converter,
    /// Counters the callback publishes into.
    metrics: Arc<OutputMetrics>,
    /// Device frames converted in one pass.
    max_block_frames: usize,
    /// `f32` staging for the integer sample formats. Empty for `f32` output.
    staging: Vec<f32>,
}

/// The buffers and the clock the conversion needs, kept apart from the mixer
/// they read so the borrow checker can see the two do not overlap.
struct Converter {
    /// The shape being converted to.
    format: NegotiatedFormat,
    /// Source frames for one pass: the carried frame followed by freshly
    /// mixed ones. Empty when the rates already agree.
    source: Vec<f32>,
    /// Device-rate frames at the *sequence* channel count, before the channel
    /// map. Empty when no channel map is needed.
    mapped: Vec<f32>,
    /// Position inside source frame 0 as `phase / device rate`, always below
    /// the device rate.
    phase: u64,
    /// How many frames at the front of `source` the previous pass left behind,
    /// mixed but not yet consumed. Zero before the first pass, which primes
    /// the stream with a frame of silence.
    held: usize,
}

impl fmt::Debug for OutputRenderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputRenderer")
            .field("format", &self.converter.format)
            .field("max_block_frames", &self.max_block_frames)
            .finish_non_exhaustive()
    }
}

impl OutputRenderer {
    /// Builds the callback body for `format`, doing every allocation here.
    ///
    /// `max_block_frames` is the longest run of device frames converted in one
    /// pass; a longer callback buffer is rendered in several passes rather
    /// than allocating.
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when `max_block_frames`
    /// is zero, and [`codes::FORMAT_UNSUPPORTED`] when the format does not
    /// describe the mixer it is given.
    pub fn new(
        mixer: Mixer,
        format: NegotiatedFormat,
        metrics: Arc<OutputMetrics>,
        max_block_frames: usize,
    ) -> SubResult<Self> {
        if max_block_frames == 0 {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "an output block must be at least one frame",
            ));
        }
        if mixer.graph().sample_rate() != format.source_sample_rate
            || mixer.graph().channels() != format.source_channels
        {
            return Err(SubError::new(
                codes::FORMAT_UNSUPPORTED,
                "the negotiated format was not derived from this mixer's graph",
            )
            .with_detail("graph_sample_rate", mixer.graph().sample_rate())
            .with_detail("graph_channels", mixer.graph().channels())
            .with_detail("source_sample_rate", format.source_sample_rate)
            .with_detail("source_channels", format.source_channels));
        }
        let source_channels = usize::from(format.source_channels);
        let device_channels = usize::from(format.channels);
        let source = if format.needs_resampling() {
            vec![0.0; max_source_frames(&format, max_block_frames)? * source_channels]
        } else {
            Vec::new()
        };
        let mapped = if format.needs_channel_map() {
            vec![0.0; max_block_frames * source_channels]
        } else {
            Vec::new()
        };
        // The integer paths always stage through `f32`, and which of the
        // render entry points the host will call is not known until it does.
        let staging = vec![0.0; max_block_frames * device_channels];
        Ok(Self {
            mixer,
            converter: Converter {
                format,
                source,
                mapped,
                phase: 0,
                held: 0,
            },
            metrics,
            max_block_frames,
            staging,
        })
    }

    /// The shape this renderer converts to.
    pub fn format(&self) -> NegotiatedFormat {
        self.converter.format
    }

    /// The counters the callback publishes into.
    pub fn metrics(&self) -> &Arc<OutputMetrics> {
        &self.metrics
    }

    /// The mixer behind the callback, for tests and for reading the transport
    /// position off the audio clock.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// Fills `out` with interleaved device frames and publishes the block's
    /// counters. Real-time safe.
    ///
    /// `out` must hold whole frames; a trailing partial frame is left silent.
    pub fn render(&mut self, out: &mut [f32]) {
        let device_channels = usize::from(self.converter.format.channels);
        let frames = out.len() / device_channels;
        let mut done = 0;
        while done < frames {
            let count = (frames - done).min(self.max_block_frames);
            let block = &mut out[done * device_channels..(done + count) * device_channels];
            self.converter.render_block(&mut self.mixer, block, count);
            done += count;
        }
        for sample in &mut out[frames * device_channels..] {
            *sample = 0.0;
        }
        self.metrics.record_block(
            u64::try_from(frames).unwrap_or(u64::MAX),
            self.mixer.underrun_frames(),
        );
    }

    /// Fills `out` with 16-bit signed device frames. Real-time safe.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the sample is clamped into i16's range before the cast"
    )]
    pub fn render_i16(&mut self, out: &mut [i16]) {
        self.render_integer(out, |sample| {
            (sample * f32::from(i16::MAX)).clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
        });
    }

    /// Fills `out` with 16-bit unsigned device frames, centred on `1 << 15`.
    /// Real-time safe.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the sample is clamped into u16's range before the cast"
    )]
    pub fn render_u16(&mut self, out: &mut [u16]) {
        self.render_integer(out, |sample| {
            let centred = sample.mul_add(f32::from(i16::MAX), 32_768.0);
            centred.clamp(0.0, f32::from(u16::MAX)) as u16
        });
    }

    /// Renders through the `f32` staging buffer and converts, a pass at a
    /// time so nothing is allocated for a long callback buffer.
    fn render_integer<T: Copy>(&mut self, out: &mut [T], convert: impl Fn(f32) -> T) {
        let device_channels = usize::from(self.converter.format.channels);
        let frames = out.len() / device_channels;
        let mut done = 0;
        while done < frames {
            let count = (frames - done).min(self.max_block_frames);
            let staging = &mut self.staging[..count * device_channels];
            self.converter.render_block(&mut self.mixer, staging, count);
            let block = &mut out[done * device_channels..(done + count) * device_channels];
            for (sample, slot) in staging.iter().zip(block.iter_mut()) {
                *slot = convert(*sample);
            }
            done += count;
        }
        self.metrics.record_block(
            u64::try_from(frames).unwrap_or(u64::MAX),
            self.mixer.underrun_frames(),
        );
    }
}

impl Converter {
    /// Renders exactly `frames` device frames from `mixer` into `block`, at
    /// the device's rate and channel count.
    fn render_block(&mut self, mixer: &mut Mixer, block: &mut [f32], frames: usize) {
        let source_channels = usize::from(self.format.source_channels);
        let device_channels = usize::from(self.format.channels);
        match (
            self.format.needs_resampling(),
            self.format.needs_channel_map(),
        ) {
            (false, false) => {
                mixer.process(block);
            }
            (false, true) => {
                let staged = &mut self.mapped[..frames * source_channels];
                mixer.process(staged);
                map_channels(staged, source_channels, block, device_channels);
            }
            (true, false) => {
                Self::resample_into(
                    mixer,
                    &self.format,
                    &mut self.source,
                    &mut self.phase,
                    &mut self.held,
                    block,
                    frames,
                );
            }
            (true, true) => {
                let staged = &mut self.mapped[..frames * source_channels];
                Self::resample_into(
                    mixer,
                    &self.format,
                    &mut self.source,
                    &mut self.phase,
                    &mut self.held,
                    staged,
                    frames,
                );
                map_channels(staged, source_channels, block, device_channels);
            }
        }
    }

    /// Converts the mixer's rate to the device's, writing `frames` frames at
    /// the *sequence* channel count into `out`.
    ///
    /// The clock is exact: for device frame `i` the source position is
    /// `(phase + i * source_rate) / device_rate`, so the whole part indexes a
    /// source frame and the remainder is the interpolation weight. Frame 0 of
    /// `source` is where the previous pass stopped, and the frames a pass
    /// mixed but did not reach are kept for the next one, so not a single
    /// source frame is skipped or read twice. The stream starts on one frame
    /// of silence, which is why the very first device frame fades in.
    #[allow(
        clippy::cast_precision_loss,
        reason = "the interpolation weight is a ratio of two sample rates, both far inside f32"
    )]
    fn resample_into(
        mixer: &mut Mixer,
        format: &NegotiatedFormat,
        source: &mut [f32],
        phase: &mut u64,
        held: &mut usize,
        out: &mut [f32],
        frames: usize,
    ) {
        let channels = usize::from(format.source_channels);
        let source_rate = u64::from(format.source_sample_rate);
        let device_rate = u64::from(format.sample_rate);
        if frames == 0 {
            return;
        }
        if *held == 0 {
            // Nothing has been mixed yet; the frame the stream starts on is
            // silence.
            source[..channels].fill(0.0);
            *held = 1;
        }
        // Two indices bound the pass: the last source frame it interpolates
        // between, and the frame the next pass will start on. Whichever is
        // further decides how much has to be mixed; anything mixed beyond
        // where the pass stops is held for the next one.
        let frames_u64 = u64::try_from(frames).unwrap_or(u64::MAX);
        let last = (*phase + (frames_u64 - 1) * source_rate) / device_rate;
        let end = (*phase + frames_u64 * source_rate) / device_rate;
        let needed = usize::try_from(last.saturating_add(1).max(end).saturating_add(1))
            .unwrap_or(usize::MAX);
        let needed = needed.min(source.len() / channels).max(*held);
        if needed > *held {
            mixer.process(&mut source[*held * channels..needed * channels]);
            *held = needed;
        }

        for (index, frame) in out.chunks_exact_mut(channels).enumerate().take(frames) {
            let position = *phase + u64::try_from(index).unwrap_or(u64::MAX) * source_rate;
            let whole = usize::try_from(position / device_rate).unwrap_or(usize::MAX);
            let weight = (position % device_rate) as f32 / device_rate as f32;
            let near = whole * channels;
            let far = near + channels;
            for (channel, sample) in frame.iter_mut().enumerate() {
                let a = source.get(near + channel).copied().unwrap_or(0.0);
                let b = source.get(far + channel).copied().unwrap_or(0.0);
                *sample = (b - a).mul_add(weight, a);
            }
        }

        // Advance past the block and shuffle everything the pass did not
        // consume down to the front for the next one.
        let position = *phase + frames_u64 * source_rate;
        let consumed = usize::try_from(position / device_rate)
            .unwrap_or(usize::MAX)
            .min(*held);
        *phase = position % device_rate;
        source.copy_within(consumed * channels..*held * channels, 0);
        *held -= consumed;
    }
}

/// The largest number of source frames one pass of `max_block_frames` device
/// frames can read, including the carried frame and the last interpolation
/// partner.
fn max_source_frames(format: &NegotiatedFormat, max_block_frames: usize) -> SubResult<usize> {
    let source_rate = u64::from(format.source_sample_rate);
    let device_rate = u64::from(format.sample_rate);
    let frames = u64::try_from(max_block_frames).unwrap_or(u64::MAX);
    // The worst case starts a hair below the next source frame, and needs
    // room for both the last interpolation partner and the carried frame.
    let phase = device_rate - 1;
    let last = (phase + frames.saturating_sub(1).saturating_mul(source_rate)) / device_rate;
    let end = (phase + frames.saturating_mul(source_rate)) / device_rate;
    usize::try_from(last.max(end).saturating_add(2)).map_err(|_| {
        SubError::new(
            sub_core::codes::INVALID_ARGUMENT,
            "the output block is too long to convert on this platform",
        )
        .with_detail("max_block_frames", max_block_frames)
    })
}

/// How a stream is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputOptions {
    /// A fixed device buffer in frames, or `None` for the host's own choice.
    pub buffer_frames: Option<u32>,
    /// The longest run of device frames the callback converts in one pass.
    pub max_block_frames: usize,
    /// How long to wait for the host to start the stream.
    pub open_timeout: Option<Duration>,
}

impl Default for OutputOptions {
    /// The host's own buffer size, 2048-frame conversion passes and a five
    /// second limit on opening, so a wedged host surfaces as an error rather
    /// than a hang.
    fn default() -> Self {
        Self {
            buffer_frames: None,
            max_block_frames: 2_048,
            open_timeout: Some(Duration::from_secs(5)),
        }
    }
}

/// A live stream, kept alive as long as this handle is.
pub trait StreamHandle {
    /// Starts or resumes playback.
    ///
    /// # Errors
    ///
    /// Returns [`codes::STREAM_FAILED`] when the host refuses.
    fn play(&self) -> SubResult<()>;

    /// Pauses playback without closing the device.
    ///
    /// # Errors
    ///
    /// Returns [`codes::STREAM_FAILED`] when the host refuses.
    fn pause(&self) -> SubResult<()>;
}

/// What a backend hands back when it opens a stream.
pub struct OpenStream {
    /// The device the stream runs on.
    pub device: OutputDeviceInfo,
    /// The shape it negotiated.
    pub format: NegotiatedFormat,
    /// The counters its callback publishes into.
    pub metrics: Arc<OutputMetrics>,
    /// The live stream.
    pub handle: Box<dyn StreamHandle>,
}

impl fmt::Debug for OpenStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenStream")
            .field("device", &self.device)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

/// The host side of the output stage: what devices exist and how a stream is
/// opened on one.
///
/// [`CpalBackend`] is the real implementation; tests use their own so that
/// device listing, switching and failure handling can run without hardware.
pub trait OutputBackend {
    /// The playback devices the host offers, the default one first.
    ///
    /// # Errors
    ///
    /// Returns [`codes::DEVICE_UNAVAILABLE`] when the host cannot be asked.
    fn devices(&self) -> SubResult<Vec<OutputDeviceInfo>>;

    /// Opens a stream on `device_id`, or on the host's default device when it
    /// is `None`, fed by `mixer`.
    ///
    /// # Errors
    ///
    /// Returns [`codes::NO_OUTPUT_DEVICE`] when there is no device to open,
    /// [`codes::DEVICE_UNAVAILABLE`] when the named device is gone,
    /// [`codes::FORMAT_UNSUPPORTED`] when nothing it advertises can be
    /// written, and [`codes::STREAM_FAILED`] when the host refuses the stream.
    fn open(
        &self,
        device_id: Option<&str>,
        mixer: Mixer,
        options: &OutputOptions,
    ) -> SubResult<OpenStream>;
}

/// A stream on a device, with the counters its callback publishes.
pub struct OutputStream {
    /// The device it runs on.
    device: OutputDeviceInfo,
    /// The shape it runs at.
    format: NegotiatedFormat,
    /// The counters its callback publishes into.
    metrics: Arc<OutputMetrics>,
    /// The live stream; dropping it closes the device.
    handle: Box<dyn StreamHandle>,
}

impl fmt::Debug for OutputStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputStream")
            .field("device", &self.device)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl OutputStream {
    /// Opens a stream through `backend`.
    ///
    /// # Errors
    ///
    /// See [`OutputBackend::open`].
    pub fn open<B: OutputBackend + ?Sized>(
        backend: &B,
        device_id: Option<&str>,
        mixer: Mixer,
        options: &OutputOptions,
    ) -> SubResult<Self> {
        let open = backend.open(device_id, mixer, options)?;
        Ok(Self {
            device: open.device,
            format: open.format,
            metrics: open.metrics,
            handle: open.handle,
        })
    }

    /// The device the stream runs on.
    pub fn device(&self) -> &OutputDeviceInfo {
        &self.device
    }

    /// The shape it runs at.
    pub fn format(&self) -> NegotiatedFormat {
        self.format
    }

    /// The counters its callback publishes into.
    pub fn metrics(&self) -> &Arc<OutputMetrics> {
        &self.metrics
    }

    /// Starts or resumes playback.
    ///
    /// # Errors
    ///
    /// Returns [`codes::STREAM_FAILED`] when the host refuses.
    pub fn play(&self) -> SubResult<()> {
        self.handle.play()
    }

    /// Pauses playback without closing the device.
    ///
    /// # Errors
    ///
    /// Returns [`codes::STREAM_FAILED`] when the host refuses.
    pub fn pause(&self) -> SubResult<()> {
        self.handle.pause()
    }

    /// A snapshot of what the stream is doing.
    pub fn diagnostics(&self) -> OutputDiagnostics {
        OutputDiagnostics {
            device_name: self.device.name.clone(),
            device_id: self.device.id.clone(),
            format: self.format,
            underrun_frames: self.metrics.underrun_frames(),
            frames_rendered: self.metrics.frames_rendered(),
            callbacks: self.metrics.callbacks(),
            stream_errors: self.metrics.stream_errors(),
            last_error: self.metrics.last_error(),
        }
    }
}

/// Builds the mixer half of a stream. Called once per open, on the engine
/// thread, because a reopened stream needs a fresh [`Mixer`] paired with a
/// fresh `MixerControl`.
///
/// # Errors
///
/// Whatever building the mixer fails with.
pub type MixerFactory = Box<dyn FnMut() -> SubResult<Mixer>>;

/// Owns the current output stream and the user's device choice.
///
/// Selecting a device closes the current stream and opens the new one with a
/// fresh mixer. When the new device cannot be opened the previous one is
/// reopened, so a bad choice in the settings panel leaves playback where it
/// was instead of taking the editor down; only if that also fails does the
/// output stay closed, and even then the handle is still usable.
pub struct AudioOutput<B: OutputBackend> {
    /// The host.
    backend: B,
    /// How streams are opened on it.
    options: OutputOptions,
    /// Makes the mixer each open needs.
    factory: MixerFactory,
    /// The device the user picked, or `None` for the host's default.
    selected: Option<String>,
    /// The stream, when one is open.
    stream: Option<OutputStream>,
}

impl<B: OutputBackend> fmt::Debug for AudioOutput<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioOutput")
            .field("selected", &self.selected)
            .field("stream", &self.stream)
            .finish_non_exhaustive()
    }
}

impl<B: OutputBackend> AudioOutput<B> {
    /// A closed output on `backend`, which will build its mixer with
    /// `factory` each time it opens a stream.
    pub fn new(backend: B, options: OutputOptions, factory: MixerFactory) -> Self {
        Self {
            backend,
            options,
            factory,
            selected: None,
            stream: None,
        }
    }

    /// The playback devices the host offers.
    ///
    /// # Errors
    ///
    /// See [`OutputBackend::devices`].
    pub fn devices(&self) -> SubResult<Vec<OutputDeviceInfo>> {
        self.backend.devices()
    }

    /// The device the user picked, or `None` for the host's default.
    pub fn selected_device(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The open stream, when there is one.
    pub fn stream(&self) -> Option<&OutputStream> {
        self.stream.as_ref()
    }

    /// Whether a stream is open.
    pub fn is_open(&self) -> bool {
        self.stream.is_some()
    }

    /// A snapshot of the open stream, when there is one.
    pub fn diagnostics(&self) -> Option<OutputDiagnostics> {
        self.stream.as_ref().map(OutputStream::diagnostics)
    }

    /// Opens a stream on the selected device and starts it.
    ///
    /// Reopening an already-open output closes the old stream first.
    ///
    /// # Errors
    ///
    /// Whatever the mixer factory or [`OutputBackend::open`] fails with. The
    /// output is left closed.
    pub fn start(&mut self) -> SubResult<()> {
        self.stream = None;
        let mixer = (self.factory)()?;
        let stream = OutputStream::open(
            &self.backend,
            self.selected.as_deref(),
            mixer,
            &self.options,
        )?;
        stream.play()?;
        tracing::info!(
            target: "sub_audio::output",
            device = %stream.device.name,
            format = %stream.format,
            "audio output opened"
        );
        self.stream = Some(stream);
        Ok(())
    }

    /// Closes the stream, if one is open.
    pub fn stop(&mut self) {
        self.stream = None;
    }

    /// Picks a device and reopens the stream on it.
    ///
    /// A closed output only records the choice. An open one is reopened: on
    /// failure the previous device is restored and its error returned, and if
    /// that fails too the output is left closed with the *first* error, which
    /// is the one that describes the device the user asked for.
    ///
    /// # Errors
    ///
    /// Whatever opening the chosen device failed with.
    pub fn select_device(&mut self, device_id: Option<&str>) -> SubResult<()> {
        let previous = self.selected.take();
        self.selected = device_id.map(str::to_owned);
        if self.stream.is_none() {
            return Ok(());
        }
        match self.start() {
            Ok(()) => Ok(()),
            Err(error) => {
                tracing::warn!(
                    target: "sub_audio::output",
                    device = device_id.unwrap_or("<default>"),
                    error = %error,
                    "could not open the chosen audio device; restoring the previous one"
                );
                self.selected = previous;
                if let Err(restore) = self.start() {
                    tracing::error!(
                        target: "sub_audio::output",
                        error = %restore,
                        "could not restore the previous audio device; output is closed"
                    );
                }
                Err(error)
            }
        }
    }
}

/// The real host: cpal's default host for the platform.
///
/// ALSA or PipeWire on Linux, WASAPI on Windows, `CoreAudio` on macOS.
pub struct CpalBackend {
    /// The host cpal picked for this platform.
    host: cpal::Host,
}

impl fmt::Debug for CpalBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpalBackend").finish_non_exhaustive()
    }
}

impl Default for CpalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CpalBackend {
    /// The platform's default host.
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    /// The id the rest of the app stores for `device`: the host's stable id
    /// where there is one, otherwise the device's name.
    fn device_id(device: &cpal::Device) -> String {
        device
            .id()
            .map_or_else(|_| device.to_string(), |id| id.id().to_string())
    }

    /// Describes one cpal device, keeping only the shapes this build writes.
    fn describe(device: &cpal::Device, default_id: Option<&str>) -> SubResult<OutputDeviceInfo> {
        let id = Self::device_id(device);
        let formats = device
            .supported_output_configs()
            .map_err(|error| {
                stream_error(
                    codes::DEVICE_UNAVAILABLE,
                    "could not ask the device what it supports",
                    &error,
                )
            })?
            .filter_map(|range| {
                sample_format(range.sample_format()).map(|format| {
                    SupportedFormat::new(
                        range.channels(),
                        range.min_sample_rate(),
                        range.max_sample_rate(),
                        format,
                    )
                })
            })
            .collect();
        Ok(OutputDeviceInfo::new(&id, device.to_string(), formats)
            .with_default(default_id == Some(id.as_str()))
            .with_default_sample_rate(device.default_output_config().ok().map(|c| c.sample_rate())))
    }

    /// The cpal device for `device_id`, or the host's default.
    fn resolve(&self, device_id: Option<&str>) -> SubResult<cpal::Device> {
        match device_id {
            None => self.host.default_output_device().ok_or_else(|| {
                SubError::new(
                    codes::NO_OUTPUT_DEVICE,
                    "this system offers no audio output device",
                )
            }),
            Some(wanted) => {
                let devices = self.host.output_devices().map_err(|error| {
                    stream_error(
                        codes::DEVICE_UNAVAILABLE,
                        "could not list the audio output devices",
                        &error,
                    )
                })?;
                devices
                    .into_iter()
                    .find(|device| Self::device_id(device) == wanted)
                    .ok_or_else(|| {
                        SubError::new(
                            codes::DEVICE_UNAVAILABLE,
                            "the chosen audio output device is not available",
                        )
                        .with_detail("device_id", wanted)
                    })
            }
        }
    }
}

/// The [`OutputSampleFormat`] for a cpal format, or `None` when this build
/// does not write it.
fn sample_format(format: cpal::SampleFormat) -> Option<OutputSampleFormat> {
    match format {
        cpal::SampleFormat::F32 => Some(OutputSampleFormat::F32),
        cpal::SampleFormat::I16 => Some(OutputSampleFormat::I16),
        cpal::SampleFormat::U16 => Some(OutputSampleFormat::U16),
        _ => None,
    }
}

/// A [`SubError`] carrying a cpal failure's kind and message.
fn stream_error(code: sub_core::ErrorCode, message: &str, error: &cpal::Error) -> SubError {
    SubError::new(code, message.to_owned())
        .with_detail("host_error", error.to_string())
        .with_detail("host_error_kind", format!("{:?}", error.kind()))
}

impl OutputBackend for CpalBackend {
    fn devices(&self) -> SubResult<Vec<OutputDeviceInfo>> {
        let default_id = self
            .host
            .default_output_device()
            .map(|d| Self::device_id(&d));
        let devices = self.host.output_devices().map_err(|error| {
            stream_error(
                codes::DEVICE_UNAVAILABLE,
                "could not list the audio output devices",
                &error,
            )
        })?;
        let mut listed: Vec<OutputDeviceInfo> = devices
            .into_iter()
            .filter_map(|device| Self::describe(&device, default_id.as_deref()).ok())
            .collect();
        // The default device first; the rest keep the host's order.
        listed.sort_by_key(|device| u8::from(!device.is_default()));
        Ok(listed)
    }

    fn open(
        &self,
        device_id: Option<&str>,
        mixer: Mixer,
        options: &OutputOptions,
    ) -> SubResult<OpenStream> {
        let device = self.resolve(device_id)?;
        let default_id = self
            .host
            .default_output_device()
            .map(|d| Self::device_id(&d));
        let info = Self::describe(&device, default_id.as_deref())?;
        let format = info.negotiate(mixer.graph().sample_rate(), mixer.graph().channels())?;
        let metrics = Arc::new(OutputMetrics::new());
        let mut renderer = OutputRenderer::new(
            mixer,
            format,
            Arc::clone(&metrics),
            options.max_block_frames,
        )?;
        let config = cpal::StreamConfig {
            channels: format.channels,
            sample_rate: format.sample_rate,
            buffer_size: options
                .buffer_frames
                .map_or(cpal::BufferSize::Default, cpal::BufferSize::Fixed),
        };
        let errors = Arc::clone(&metrics);
        let on_error = move |error: cpal::Error| errors.record_stream_error(error.to_string());
        let stream = match format.sample_format {
            OutputSampleFormat::F32 => device.build_output_stream(
                config,
                move |data: &mut [f32], _| renderer.render(data),
                on_error,
                options.open_timeout,
            ),
            OutputSampleFormat::I16 => device.build_output_stream(
                config,
                move |data: &mut [i16], _| renderer.render_i16(data),
                on_error,
                options.open_timeout,
            ),
            OutputSampleFormat::U16 => device.build_output_stream(
                config,
                move |data: &mut [u16], _| renderer.render_u16(data),
                on_error,
                options.open_timeout,
            ),
        }
        .map_err(|error| {
            stream_error(
                codes::STREAM_FAILED,
                "the audio host would not open an output stream",
                &error,
            )
        })?;
        Ok(OpenStream {
            device: info,
            format,
            metrics,
            handle: Box::new(CpalStreamHandle(stream)),
        })
    }
}

/// Keeps a cpal stream alive and starts and stops it.
struct CpalStreamHandle(cpal::Stream);

impl StreamHandle for CpalStreamHandle {
    fn play(&self) -> SubResult<()> {
        self.0.play().map_err(|error| {
            stream_error(
                codes::STREAM_FAILED,
                "the audio host would not start the output stream",
                &error,
            )
        })
    }

    fn pause(&self) -> SubResult<()> {
        self.0.pause().map_err(|error| {
            stream_error(
                codes::STREAM_FAILED,
                "the audio host would not pause the output stream",
                &error,
            )
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    reason = "test signals are small integers, exact in f32 and f64"
)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use sub_time::{Rational, RationalTime};

    use crate::mixer::{ClipSpec, MixGraphBuilder, MixerConfig, TrackSpec, mixer};
    use crate::resample::{PcmWriter, pcm_ring};

    use super::*;

    /// A mixer whose single clip plays whatever is written into `writer`.
    fn test_mixer(sample_rate: u32, channels: u16) -> (PcmWriter, Mixer) {
        let rate = Rational::from_integer(sample_rate).expect("a valid rate");
        let graph = MixGraphBuilder::new(sample_rate, channels)
            .track(TrackSpec::new().with_clip(ClipSpec::new(
                0,
                RationalTime::zero(rate),
                RationalTime::new(i64::from(sample_rate) * 60, rate),
            )))
            .build()
            .expect("a valid graph");
        let (mut control, mixer) = mixer(graph, MixerConfig::default()).expect("a mixer");
        let (writer, reader) = pcm_ring(channels, 1 << 16).expect("a ring");
        // The callback takes the slot at the top of its first block, so there
        // is nothing to prime here.
        control.install_slot(0, reader).expect("a free slot");
        (writer, mixer)
    }

    fn make_renderer(
        sample_rate: u32,
        channels: u16,
        format: NegotiatedFormat,
    ) -> (PcmWriter, OutputRenderer) {
        let (writer, mixer) = test_mixer(sample_rate, channels);
        let renderer = OutputRenderer::new(mixer, format, Arc::new(OutputMetrics::new()), 256)
            .expect("a renderer");
        (writer, renderer)
    }

    fn device_format(
        device_rate: u32,
        device_channels: u16,
        source_rate: u32,
        source_channels: u16,
    ) -> NegotiatedFormat {
        NegotiatedFormat {
            sample_rate: device_rate,
            channels: device_channels,
            sample_format: OutputSampleFormat::F32,
            source_sample_rate: source_rate,
            source_channels,
        }
    }

    #[test]
    fn an_exact_rate_is_preferred_over_a_closer_format() {
        let supported = [
            SupportedFormat::new(2, 44_100, 44_100, OutputSampleFormat::F32),
            SupportedFormat::new(2, 48_000, 48_000, OutputSampleFormat::I16),
        ];
        let chosen = negotiate(&supported, 48_000, 2).expect("a format");
        assert_eq!(chosen.sample_rate(), 48_000);
        assert_eq!(chosen.sample_format(), OutputSampleFormat::I16);
        assert!(!chosen.needs_resampling());
        assert!(!chosen.needs_channel_map());
    }

    #[test]
    fn a_rate_inside_a_range_is_taken_exactly() {
        let supported = [SupportedFormat::new(
            2,
            8_000,
            192_000,
            OutputSampleFormat::F32,
        )];
        let chosen = negotiate(&supported, 48_000, 2).expect("a format");
        assert_eq!(chosen.sample_rate(), 48_000);
        assert!(!chosen.needs_resampling());
    }

    #[test]
    fn the_closest_rate_wins_when_none_is_exact() {
        let supported = [
            SupportedFormat::new(2, 96_000, 96_000, OutputSampleFormat::F32),
            SupportedFormat::new(2, 44_100, 44_100, OutputSampleFormat::F32),
        ];
        let chosen = negotiate(&supported, 48_000, 2).expect("a format");
        assert_eq!(chosen.sample_rate(), 44_100);
        assert!(chosen.needs_resampling());
        assert_eq!(chosen.source_sample_rate(), 48_000);
    }

    #[test]
    fn f32_wins_a_tie_between_sample_formats() {
        let supported = [
            SupportedFormat::new(2, 48_000, 48_000, OutputSampleFormat::U16),
            SupportedFormat::new(2, 48_000, 48_000, OutputSampleFormat::F32),
            SupportedFormat::new(2, 48_000, 48_000, OutputSampleFormat::I16),
        ];
        let chosen = negotiate(&supported, 48_000, 2).expect("a format");
        assert_eq!(chosen.sample_format(), OutputSampleFormat::F32);
    }

    #[test]
    fn an_exact_channel_count_beats_a_wider_one() {
        let supported = [
            SupportedFormat::new(8, 48_000, 48_000, OutputSampleFormat::F32),
            SupportedFormat::new(2, 48_000, 48_000, OutputSampleFormat::F32),
        ];
        let chosen = negotiate(&supported, 48_000, 2).expect("a format");
        assert_eq!(chosen.channels(), 2);
    }

    #[test]
    fn a_wider_device_beats_a_narrower_one() {
        let supported = [
            SupportedFormat::new(1, 48_000, 48_000, OutputSampleFormat::F32),
            SupportedFormat::new(4, 48_000, 48_000, OutputSampleFormat::F32),
        ];
        let chosen = negotiate(&supported, 48_000, 2).expect("a format");
        assert_eq!(chosen.channels(), 4);
        assert!(chosen.needs_channel_map());
    }

    #[test]
    fn a_device_with_no_writable_format_is_rejected() {
        let error = negotiate(&[], 48_000, 2).expect_err("no format");
        assert_eq!(error.code, codes::FORMAT_UNSUPPORTED);
    }

    #[test]
    fn a_nonsense_sequence_shape_is_rejected() {
        let supported = [SupportedFormat::new(
            2,
            48_000,
            48_000,
            OutputSampleFormat::F32,
        )];
        for (rate, channels) in [(0, 2), (48_000, 0), (48_000, MAX_CHANNELS + 1)] {
            let error = negotiate(&supported, rate, channels).expect_err("an invalid shape");
            assert_eq!(error.code, sub_core::codes::INVALID_ARGUMENT);
        }
    }

    #[test]
    fn mono_fans_out_to_the_first_two_channels() {
        let src = [0.5, -0.25];
        let mut dst = [9.0; 8];
        map_channels(&src, 1, &mut dst, 4);
        assert_eq!(
            dst.to_vec(),
            vec![0.5, 0.5, 0.0, 0.0, -0.25, -0.25, 0.0, 0.0]
        );
    }

    #[test]
    fn a_mono_device_sums_the_sequence_down() {
        let src = [1.0, 0.0, -1.0, 1.0];
        let mut dst = [9.0; 2];
        map_channels(&src, 2, &mut dst, 1);
        assert_eq!(dst.to_vec(), vec![0.5, 0.0]);
    }

    #[test]
    fn extra_device_channels_are_silent() {
        let src = [1.0, -1.0];
        let mut dst = [9.0; 6];
        map_channels(&src, 2, &mut dst, 6);
        assert_eq!(dst.to_vec(), vec![1.0, -1.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_matched_stream_passes_the_mixer_through_untouched() {
        let (mut writer, mut renderer) =
            make_renderer(48_000, 2, device_format(48_000, 2, 48_000, 2));
        let input: Vec<f32> = (0..512)
            .map(|i| if i % 2 == 0 { 0.25 } else { -0.25 })
            .collect();
        writer.write(&input);
        let mut out = vec![0.0; 256 * 2];
        renderer.render(&mut out);
        assert_eq!(out, input);
        assert_eq!(renderer.metrics().frames_rendered(), 256);
        assert_eq!(renderer.metrics().callbacks(), 1);
    }

    #[test]
    fn a_constant_signal_survives_resampling() {
        // 48 kHz sequence into a 44.1 kHz device.
        let (mut writer, mut renderer) =
            make_renderer(48_000, 2, device_format(44_100, 2, 48_000, 2));
        let input = vec![0.5; 2 * 8_192];
        writer.write(&input);
        let mut out = vec![0.0; 1_000 * 2];
        renderer.render(&mut out);
        // The first frame interpolates out of the silence the stream starts
        // from; every later frame is the constant itself.
        for sample in &out[2..] {
            assert!((sample - 0.5).abs() < 1e-6, "sample {sample} drifted");
        }
    }

    #[test]
    fn the_resampler_consumes_the_exact_rate_ratio_without_drifting() {
        // A ramp makes the source position readable straight off the output.
        let (mut writer, mut renderer) =
            make_renderer(48_000, 1, device_format(44_100, 1, 48_000, 1));
        let ramp: Vec<f32> = (0..40_000).map(|i| i as f32).collect();
        writer.write(&ramp);
        let mut out = vec![0.0; 4_000];
        renderer.render(&mut out);
        // Frame i of the device reads source position (i * 48000) / 44100,
        // less the one-frame priming delay the carried frame introduces.
        for (index, sample) in out.iter().enumerate().skip(1) {
            let expected = (index as f64 * 48_000.0 / 44_100.0) - 1.0;
            assert!(
                (f64::from(*sample) - expected).abs() < 1e-2,
                "frame {index}: expected {expected}, got {sample}"
            );
        }
    }

    #[test]
    fn upsampling_reads_fewer_source_frames_than_it_writes() {
        let (mut writer, mut renderer) =
            make_renderer(44_100, 1, device_format(48_000, 1, 44_100, 1));
        let ramp: Vec<f32> = (0..8_000).map(|i| i as f32).collect();
        writer.write(&ramp);
        let mut out = vec![0.0; 4_000];
        renderer.render(&mut out);
        for (index, sample) in out.iter().enumerate().skip(2) {
            let expected = (index as f64 * 44_100.0 / 48_000.0) - 1.0;
            assert!(
                (f64::from(*sample) - expected).abs() < 1e-2,
                "frame {index}: expected {expected}, got {sample}"
            );
        }
    }

    #[test]
    fn a_block_longer_than_one_pass_is_rendered_in_several() {
        let (mut writer, mut renderer) =
            make_renderer(48_000, 2, device_format(48_000, 2, 48_000, 2));
        let input = vec![0.5; 2 * 4_096];
        writer.write(&input);
        // 1000 frames with a 256-frame pass limit.
        let mut out = vec![0.0; 1_000 * 2];
        renderer.render(&mut out);
        assert!(out.iter().all(|sample| (sample - 0.5).abs() < 1e-6));
        assert_eq!(renderer.metrics().frames_rendered(), 1_000);
        assert_eq!(renderer.metrics().callbacks(), 1);
    }

    #[test]
    fn an_empty_ring_is_silence_and_counts_as_an_underrun() {
        let (_writer, mut renderer) = make_renderer(48_000, 2, device_format(48_000, 2, 48_000, 2));
        let mut out = vec![9.0; 256 * 2];
        renderer.render(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0));
        assert_eq!(renderer.metrics().underrun_frames(), 256);
    }

    #[test]
    fn integer_output_is_scaled_and_centred() {
        let (mut writer, mut renderer) =
            make_renderer(48_000, 1, device_format(48_000, 1, 48_000, 1));
        writer.write(&[1.0, -1.0, 0.0, 0.0]);
        let mut signed = [0i16; 2];
        renderer.render_i16(&mut signed);
        assert_eq!(signed, [i16::MAX, -i16::MAX]);

        let (mut writer, mut renderer) = make_renderer(48_000, 1, {
            let mut format = device_format(48_000, 1, 48_000, 1);
            format.sample_format = OutputSampleFormat::U16;
            format
        });
        writer.write(&[1.0, -1.0]);
        let mut unsigned = [0u16; 2];
        renderer.render_u16(&mut unsigned);
        assert_eq!(unsigned, [65_535, 1]);
    }

    #[test]
    fn a_renderer_rejects_a_format_from_another_mixer() {
        let (_writer, mixer) = test_mixer(48_000, 2);
        let error = OutputRenderer::new(
            mixer,
            device_format(48_000, 2, 44_100, 2),
            Arc::new(OutputMetrics::new()),
            256,
        )
        .expect_err("a mismatched format");
        assert_eq!(error.code, codes::FORMAT_UNSUPPORTED);
    }

    #[test]
    fn a_renderer_rejects_a_zero_length_block() {
        let (_writer, mixer) = test_mixer(48_000, 2);
        let error = OutputRenderer::new(
            mixer,
            device_format(48_000, 2, 48_000, 2),
            Arc::new(OutputMetrics::new()),
            0,
        )
        .expect_err("a zero block");
        assert_eq!(error.code, sub_core::codes::INVALID_ARGUMENT);
    }

    /// What a [`StubBackend`] did, so a test can see the switching.
    #[derive(Debug, Default)]
    struct StubLog {
        /// The device id each open asked for.
        opened: Vec<Option<String>>,
        /// Device ids that refuse to open.
        broken: Vec<String>,
    }

    /// A backend with two devices and no hardware behind it.
    struct StubBackend {
        /// What it has been asked to do.
        log: Rc<RefCell<StubLog>>,
    }

    struct StubHandle;

    impl StreamHandle for StubHandle {
        fn play(&self) -> SubResult<()> {
            Ok(())
        }

        fn pause(&self) -> SubResult<()> {
            Ok(())
        }
    }

    impl OutputBackend for StubBackend {
        fn devices(&self) -> SubResult<Vec<OutputDeviceInfo>> {
            let formats = vec![SupportedFormat::new(
                2,
                48_000,
                48_000,
                OutputSampleFormat::F32,
            )];
            Ok(vec![
                OutputDeviceInfo::new("built-in", "Built-in Output", formats.clone())
                    .with_default(true),
                OutputDeviceInfo::new("usb", "USB Interface", formats),
            ])
        }

        fn open(
            &self,
            device_id: Option<&str>,
            mixer: Mixer,
            options: &OutputOptions,
        ) -> SubResult<OpenStream> {
            self.log
                .borrow_mut()
                .opened
                .push(device_id.map(str::to_owned));
            let wanted = device_id.unwrap_or("built-in");
            if self.log.borrow().broken.iter().any(|id| id == wanted) {
                return Err(SubError::new(
                    codes::DEVICE_UNAVAILABLE,
                    "this stub device refuses to open",
                ));
            }
            let device = self
                .devices()?
                .into_iter()
                .find(|device| device.id() == wanted)
                .ok_or_else(|| SubError::new(codes::DEVICE_UNAVAILABLE, "no such stub device"))?;
            let format = device.negotiate(mixer.graph().sample_rate(), mixer.graph().channels())?;
            let metrics = Arc::new(OutputMetrics::new());
            // Building the renderer proves the mixer and the format agree.
            let _renderer = OutputRenderer::new(
                mixer,
                format,
                Arc::clone(&metrics),
                options.max_block_frames,
            )?;
            Ok(OpenStream {
                device,
                format,
                metrics,
                handle: Box::new(StubHandle),
            })
        }
    }

    fn stub_output(log: &Rc<RefCell<StubLog>>) -> AudioOutput<StubBackend> {
        let backend = StubBackend {
            log: Rc::clone(log),
        };
        AudioOutput::new(
            backend,
            OutputOptions::default(),
            Box::new(|| Ok(test_mixer(48_000, 2).1)),
        )
    }

    #[test]
    fn the_default_device_is_listed_first() {
        let log = Rc::new(RefCell::new(StubLog::default()));
        let output = stub_output(&log);
        let devices = output.devices().expect("devices");
        assert_eq!(devices.len(), 2);
        assert!(devices[0].is_default());
        assert_eq!(devices[0].id(), "built-in");
    }

    #[test]
    fn switching_devices_reopens_the_stream() {
        let log = Rc::new(RefCell::new(StubLog::default()));
        let mut output = stub_output(&log);
        output.start().expect("the default device opens");
        assert_eq!(output.stream().expect("a stream").device().id(), "built-in");

        output.select_device(Some("usb")).expect("the switch works");
        assert_eq!(output.selected_device(), Some("usb"));
        assert_eq!(output.stream().expect("a stream").device().id(), "usb");
        assert_eq!(
            log.borrow().opened,
            vec![None, Some("usb".to_owned())],
            "each selection opens exactly once"
        );
    }

    #[test]
    fn a_device_that_will_not_open_leaves_the_previous_one_playing() {
        let log = Rc::new(RefCell::new(StubLog::default()));
        log.borrow_mut().broken.push("usb".to_owned());
        let mut output = stub_output(&log);
        output.start().expect("the default device opens");

        let error = output.select_device(Some("usb")).expect_err("a bad device");
        assert_eq!(error.code, codes::DEVICE_UNAVAILABLE);
        assert_eq!(output.selected_device(), None);
        assert!(output.is_open(), "the previous device is still playing");
        assert_eq!(output.stream().expect("a stream").device().id(), "built-in");
    }

    #[test]
    fn selecting_a_device_while_closed_only_records_the_choice() {
        let log = Rc::new(RefCell::new(StubLog::default()));
        let mut output = stub_output(&log);
        output.select_device(Some("usb")).expect("recorded");
        assert!(!output.is_open());
        assert!(log.borrow().opened.is_empty());
        output.start().expect("the chosen device opens");
        assert_eq!(log.borrow().opened, vec![Some("usb".to_owned())]);
    }

    #[test]
    fn diagnostics_describe_the_open_stream() {
        let log = Rc::new(RefCell::new(StubLog::default()));
        let mut output = stub_output(&log);
        output.start().expect("a stream");
        let diagnostics = output.diagnostics().expect("a snapshot");
        assert_eq!(diagnostics.device_name, "Built-in Output");
        assert_eq!(diagnostics.format.sample_rate(), 48_000);
        assert!(diagnostics.is_healthy());
        assert!(diagnostics.to_string().contains("Built-in Output"));

        output
            .stream()
            .expect("a stream")
            .metrics()
            .record_stream_error("the device fell over");
        let diagnostics = output.diagnostics().expect("a snapshot");
        assert_eq!(diagnostics.stream_errors, 1);
        assert!(!diagnostics.is_healthy());
        assert_eq!(
            diagnostics.last_error.as_deref(),
            Some("the device fell over")
        );
    }

    #[test]
    fn stopping_closes_the_stream_but_keeps_the_choice() {
        let log = Rc::new(RefCell::new(StubLog::default()));
        let mut output = stub_output(&log);
        output.select_device(Some("usb")).expect("recorded");
        output.start().expect("a stream");
        output.stop();
        assert!(!output.is_open());
        assert_eq!(output.selected_device(), Some("usb"));
        assert!(output.diagnostics().is_none());
    }
}
