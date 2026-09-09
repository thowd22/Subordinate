//! Converting a source's sample rate to the sequence rate (docs/PLAN.md §5.4).
//!
//! A timeline mixes 44.1 kHz and 48 kHz sources on the same buses, so every
//! source that does not already run at the sequence rate gets its own
//! [`Resampler`]. The conversion is a windowed-sinc interpolation from rubato
//! with a fixed input chunk, which gives a **fixed** latency: the mixer reads
//! it once with [`Resampler::latency`] and compensates for the whole life of
//! the source rather than tracking a moving delay.
//!
//! Nothing here runs in the audio callback. A worker thread owns the
//! [`ResampleStage`], which resamples ahead of time and pushes frames into a
//! lock-free single-producer single-consumer ring; the callback owns the
//! [`PcmReader`] end and only ever copies out of it, never locking and never
//! allocating.
//!
//! ```
//! # fn main() -> sub_core::SubResult<()> {
//! use sub_audio::resample::{PcmReader, ResampleStage, Resampler, pcm_ring};
//!
//! let resampler = Resampler::new(44_100, 48_000, 2)?;
//! let latency = resampler.latency(); // exact, at the sequence rate
//! let (writer, mut reader) = pcm_ring(2, 8_192)?;
//! let mut stage = ResampleStage::new(resampler, writer)?;
//!
//! // On a worker thread: feed interleaved source frames.
//! stage.feed(&[0.0; 2 * 1_024])?;
//!
//! // In the audio callback: copy out, no lock and no allocation.
//! let mut block = [0.0f32; 2 * 256];
//! let frames = reader.read(&mut block);
//! # let _ = (latency, frames);
//! # Ok(())
//! # }
//! ```

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Indexing, Resampler as _, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};

use sub_core::{SubError, SubResult};
use sub_time::{Rational, RationalTime};

use crate::codes;
use crate::decode::MAX_CHANNELS;

/// Input frames a [`Resampler`] consumes per conversion step.
///
/// Large enough that the per-chunk bookkeeping is negligible, small enough
/// that a decoder block usually fills at least one chunk.
pub const DEFAULT_CHUNK_FRAMES: usize = 1_024;

/// Length of the windowed sinc filter, in taps.
///
/// Sets the quality and, with it, the fixed latency: the filter is centred, so
/// the delay is about half this many source frames.
const SINC_LEN: usize = 256;

/// Safety valve for [`Resampler::flush`]: silent chunks pushed through the
/// filter can never need more than this many iterations to drain the delay.
const MAX_FLUSH_CHUNKS: usize = 1_024;

/// Converts one source's interleaved `f32` frames to the sequence sample rate.
///
/// The ratio is fixed at construction, so the latency is fixed too and is
/// reported once to the mixer. When the source already runs at the sequence
/// rate the resampler is a pass-through with zero latency and no filtering.
///
/// This type allocates while converting only when the caller's output vector
/// grows, so it belongs on a worker thread, never in the audio callback.
pub struct Resampler {
    /// The rubato resampler, or `None` when source and sequence rates match.
    inner: Option<Box<Async<f32>>>,
    /// Sample rate of the source material, in hertz.
    source_rate: u32,
    /// Sample rate of the sequence being mixed, in hertz.
    sequence_rate: u32,
    /// Channels in both the input and the output; frames are interleaved.
    channels: u16,
    /// Input frames consumed per conversion step.
    chunk_frames: usize,
    /// Fixed delay through the filter, in output frames.
    latency_frames: usize,
    /// Interleaved input frames not yet forming a whole chunk.
    carry: Vec<f32>,
    /// Reusable output scratch, sized for the largest chunk rubato can emit.
    scratch: Vec<f32>,
    /// Source frames accepted so far, for the exact flush length.
    frames_in: u64,
    /// Output frames produced so far, including the leading latency.
    frames_out: u64,
}

impl std::fmt::Debug for Resampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resampler")
            .field("source_rate", &self.source_rate)
            .field("sequence_rate", &self.sequence_rate)
            .field("channels", &self.channels)
            .field("latency_frames", &self.latency_frames)
            .finish_non_exhaustive()
    }
}

impl Resampler {
    /// Builds a resampler from `source_rate` to `sequence_rate` for
    /// `channels` interleaved channels, using [`DEFAULT_CHUNK_FRAMES`].
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when either rate is zero,
    /// [`codes::UNSUPPORTED_LAYOUT`] when the channel count is zero or above
    /// [`MAX_CHANNELS`], and [`codes::RESAMPLE_FAILED`] when the filter cannot
    /// be built for this ratio.
    pub fn new(source_rate: u32, sequence_rate: u32, channels: u16) -> SubResult<Self> {
        Self::with_chunk_frames(source_rate, sequence_rate, channels, DEFAULT_CHUNK_FRAMES)
    }

    /// Builds a resampler that consumes `chunk_frames` source frames per step.
    ///
    /// # Errors
    ///
    /// The same codes as [`Resampler::new`], plus
    /// [`sub_core::codes::INVALID_ARGUMENT`] when `chunk_frames` is zero.
    pub fn with_chunk_frames(
        source_rate: u32,
        sequence_rate: u32,
        channels: u16,
        chunk_frames: usize,
    ) -> SubResult<Self> {
        if source_rate == 0 || sequence_rate == 0 {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "a sample rate of zero cannot be resampled",
            )
            .with_detail("source_rate", source_rate)
            .with_detail("sequence_rate", sequence_rate));
        }
        if channels == 0 || channels > MAX_CHANNELS {
            return Err(SubError::new(
                codes::UNSUPPORTED_LAYOUT,
                "channel count is outside what a mixer bus can hold",
            )
            .with_detail("channels", channels)
            .with_detail("max_channels", MAX_CHANNELS));
        }
        if chunk_frames == 0 {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "a resampler chunk must hold at least one frame",
            ));
        }

        if source_rate == sequence_rate {
            return Ok(Self {
                inner: None,
                source_rate,
                sequence_rate,
                channels,
                chunk_frames,
                latency_frames: 0,
                carry: Vec::new(),
                scratch: Vec::new(),
                frames_in: 0,
                frames_out: 0,
            });
        }

        let ratio = f64::from(sequence_rate) / f64::from(source_rate);
        let parameters =
            SincInterpolationParameters::new(SINC_LEN, WindowFunction::BlackmanHarris2)
                .interpolation(SincInterpolationType::Cubic)
                .oversampling_factor(256);
        let inner = Async::<f32>::new_sinc(
            ratio,
            1.0,
            &parameters,
            chunk_frames,
            usize::from(channels),
            FixedAsync::Input,
        )
        .map_err(|e| {
            SubError::wrap(
                codes::RESAMPLE_FAILED,
                "could not build a resampler for this rate change",
                &e,
            )
            .with_detail("source_rate", source_rate)
            .with_detail("sequence_rate", sequence_rate)
        })?;

        let latency_frames = inner.output_delay();
        let scratch = vec![0.0; inner.output_frames_max() * usize::from(channels)];
        tracing::debug!(
            source_rate,
            sequence_rate,
            channels,
            latency_frames,
            "built a source resampler"
        );
        Ok(Self {
            inner: Some(Box::new(inner)),
            source_rate,
            sequence_rate,
            channels,
            chunk_frames,
            latency_frames,
            carry: Vec::new(),
            scratch,
            frames_in: 0,
            frames_out: 0,
        })
    }

    /// Sample rate of the source material, in hertz.
    pub fn source_rate(&self) -> u32 {
        self.source_rate
    }

    /// Sample rate this resampler converts to, in hertz.
    pub fn sequence_rate(&self) -> u32 {
        self.sequence_rate
    }

    /// Channel count of both the input and the output.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// True when source and sequence rates match and frames pass through
    /// untouched.
    pub fn is_passthrough(&self) -> bool {
        self.inner.is_none()
    }

    /// Fixed delay through the filter, counted in output frames.
    ///
    /// The first `latency_frames` output frames are the filter's leading
    /// ramp-in rather than material; the mixer compensates by this amount.
    pub fn latency_frames(&self) -> usize {
        self.latency_frames
    }

    /// The fixed latency as an exact [`RationalTime`] at the sequence rate.
    ///
    /// This is what the mixer is told once, at construction; it never changes
    /// while the source plays.
    ///
    /// # Panics
    ///
    /// Never: the sequence rate is checked to be non-zero when the resampler
    /// is built, and a non-zero numerator over one is always a valid rational.
    pub fn latency(&self) -> RationalTime {
        RationalTime::new(
            i64::try_from(self.latency_frames).unwrap_or(i64::MAX),
            self.rate(),
        )
    }

    /// The sequence rate as a [`Rational`]: one unit per output frame.
    ///
    /// # Panics
    ///
    /// Never: the sequence rate is checked to be non-zero when the resampler
    /// is built.
    pub fn rate(&self) -> Rational {
        Rational::new(self.sequence_rate, 1).expect("a checked sample rate is non-zero")
    }

    /// Frames held back because they do not yet fill a whole chunk.
    pub fn buffered_frames(&self) -> usize {
        self.carry.len() / usize::from(self.channels)
    }

    /// Converts `input` and appends the interleaved result to `out`.
    ///
    /// Input that does not fill a whole chunk is held until the next call, so
    /// splitting a stream into arbitrary chunks produces exactly the same
    /// output as converting it in one piece: there is no discontinuity at a
    /// chunk boundary.
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when `input` does not
    /// hold whole interleaved frames, and [`codes::RESAMPLE_FAILED`] when the
    /// filter rejects a chunk.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> SubResult<()> {
        let channels = usize::from(self.channels);
        if !input.len().is_multiple_of(channels) {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "input is not a whole number of interleaved frames",
            )
            .with_detail("samples", input.len())
            .with_detail("channels", self.channels));
        }
        self.frames_in = self
            .frames_in
            .saturating_add(u64::try_from(input.len() / channels).unwrap_or(0));

        if self.inner.is_none() {
            out.extend_from_slice(input);
            self.frames_out = self
                .frames_out
                .saturating_add(u64::try_from(input.len() / channels).unwrap_or(0));
            return Ok(());
        }

        self.carry.extend_from_slice(input);
        let chunk = self.chunk_frames * channels;
        let mut consumed = 0;
        while self.carry.len() - consumed >= chunk {
            let end = consumed + chunk;
            self.convert_chunk(consumed..end, self.chunk_frames, out)?;
            consumed = end;
        }
        self.carry.drain(..consumed);
        Ok(())
    }

    /// Pushes the last partial chunk and the filter's tail through, appending
    /// to `out`, and leaves the resampler ready to be dropped.
    ///
    /// After flushing, `out` has received exactly
    /// `latency_frames + source_frames * sequence_rate / source_rate` frames
    /// in total across every call, computed in integer arithmetic.
    ///
    /// # Errors
    ///
    /// Returns [`codes::RESAMPLE_FAILED`] when the filter rejects a chunk.
    pub fn flush(&mut self, out: &mut Vec<f32>) -> SubResult<()> {
        if self.inner.is_none() {
            return Ok(());
        }
        let channels = usize::from(self.channels);
        let partial = self.carry.len() / channels;
        if partial > 0 {
            let end = self.carry.len();
            self.convert_chunk(0..end, partial, out)?;
            self.carry.clear();
        }

        let expected = self.expected_output_frames();
        let mut chunks = 0;
        while self.frames_out < expected && chunks < MAX_FLUSH_CHUNKS {
            self.convert_chunk(0..0, 0, out)?;
            chunks += 1;
        }
        if let Some(extra) = self.frames_out.checked_sub(expected)
            && let Ok(extra) = usize::try_from(extra)
        {
            let keep = out.len().saturating_sub(extra * channels);
            out.truncate(keep);
            self.frames_out = expected;
        }
        Ok(())
    }

    /// Total output frames a whole clip is worth, latency included, in exact
    /// integer arithmetic.
    fn expected_output_frames(&self) -> u64 {
        let scaled = u128::from(self.frames_in) * u128::from(self.sequence_rate)
            / u128::from(self.source_rate);
        let frames = u64::try_from(scaled).unwrap_or(u64::MAX);
        frames.saturating_add(u64::try_from(self.latency_frames).unwrap_or(0))
    }

    /// Runs one conversion step over `range` of the carry buffer, treating
    /// `valid_frames` of it as real material and the rest as silence.
    fn convert_chunk(
        &mut self,
        range: std::ops::Range<usize>,
        valid_frames: usize,
        out: &mut Vec<f32>,
    ) -> SubResult<()> {
        let channels = usize::from(self.channels);
        let inner = self
            .inner
            .as_mut()
            .expect("convert_chunk is never reached in pass-through mode");

        // A partial or silent chunk still reads `chunk_frames` frames, so the
        // adapter is given a full-length window backed by zero padding.
        let mut padded;
        let input: &[f32] = if range.len() == self.chunk_frames * channels {
            &self.carry[range]
        } else {
            padded = vec![0.0; self.chunk_frames * channels];
            padded[..range.len()].copy_from_slice(&self.carry[range]);
            &padded
        };

        let adapter = InterleavedSlice::new(input, channels, self.chunk_frames).map_err(|e| {
            SubError::wrap(codes::RESAMPLE_FAILED, "resampler input is misshapen", &e)
        })?;
        let frames_out = inner.output_frames_next();
        if self.scratch.len() < frames_out * channels {
            self.scratch.resize(frames_out * channels, 0.0);
        }
        let mut sink = InterleavedSlice::new_mut(self.scratch.as_mut_slice(), channels, frames_out)
            .map_err(|e| {
                SubError::wrap(codes::RESAMPLE_FAILED, "resampler output is misshapen", &e)
            })?;
        let indexing = Indexing::new().partial_len(valid_frames);
        let (_, written) = inner
            .process_into_buffer(&adapter, &mut sink, Some(&indexing))
            .map_err(|e| {
                SubError::wrap(codes::RESAMPLE_FAILED, "resampling a chunk failed", &e)
                    .with_detail("source_rate", self.source_rate)
                    .with_detail("sequence_rate", self.sequence_rate)
            })?;
        out.extend_from_slice(&self.scratch[..written * channels]);
        self.frames_out = self
            .frames_out
            .saturating_add(u64::try_from(written).unwrap_or(0));
        Ok(())
    }
}

/// Creates a lock-free ring holding `capacity_frames` interleaved frames.
///
/// The writer belongs to the worker thread that resamples; the reader belongs
/// to the audio callback, which only copies out of it and never locks or
/// allocates.
///
/// # Errors
///
/// Returns [`codes::UNSUPPORTED_LAYOUT`] when the channel count is zero or
/// above [`MAX_CHANNELS`], and [`sub_core::codes::INVALID_ARGUMENT`] when the
/// capacity is zero.
pub fn pcm_ring(channels: u16, capacity_frames: usize) -> SubResult<(PcmWriter, PcmReader)> {
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(SubError::new(
            codes::UNSUPPORTED_LAYOUT,
            "channel count is outside what a mixer bus can hold",
        )
        .with_detail("channels", channels));
    }
    if capacity_frames == 0 {
        return Err(SubError::new(
            sub_core::codes::INVALID_ARGUMENT,
            "a pcm ring must hold at least one frame",
        ));
    }
    let (producer, consumer) = HeapRb::<f32>::new(capacity_frames * usize::from(channels)).split();
    Ok((
        PcmWriter {
            producer,
            channels,
            capacity_frames,
        },
        PcmReader {
            consumer,
            channels,
            capacity_frames,
        },
    ))
}

/// The producing end of a PCM ring: the worker thread writes resampled frames.
pub struct PcmWriter {
    /// Lock-free single-producer handle over interleaved samples.
    producer: ringbuf::HeapProd<f32>,
    /// Channels per frame.
    channels: u16,
    /// Frames the ring can hold when empty.
    capacity_frames: usize,
}

impl std::fmt::Debug for PcmWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PcmWriter")
            .field("channels", &self.channels)
            .field("capacity_frames", &self.capacity_frames)
            .field("vacant_frames", &self.vacant_frames())
            .finish_non_exhaustive()
    }
}

impl PcmWriter {
    /// Writes as many whole frames of `interleaved` as fit, returning how many
    /// frames were taken. A partial frame is never written.
    pub fn write(&mut self, interleaved: &[f32]) -> usize {
        let channels = usize::from(self.channels);
        let offered = interleaved.len() / channels;
        let room = self.producer.vacant_len() / channels;
        let frames = offered.min(room);
        if frames == 0 {
            return 0;
        }
        self.producer.push_slice(&interleaved[..frames * channels]);
        frames
    }

    /// Frames that can be written before the ring is full.
    pub fn vacant_frames(&self) -> usize {
        self.producer.vacant_len() / usize::from(self.channels)
    }

    /// Frames the ring holds when empty.
    pub fn capacity_frames(&self) -> usize {
        self.capacity_frames
    }

    /// Channels per frame.
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

/// The consuming end of a PCM ring: the audio callback reads frames from it.
///
/// Every method here is wait-free and allocation-free, so it is safe to call
/// from the real-time callback.
pub struct PcmReader {
    /// Lock-free single-consumer handle over interleaved samples.
    consumer: ringbuf::HeapCons<f32>,
    /// Channels per frame.
    channels: u16,
    /// Frames the ring can hold when empty.
    capacity_frames: usize,
}

impl std::fmt::Debug for PcmReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PcmReader")
            .field("channels", &self.channels)
            .field("capacity_frames", &self.capacity_frames)
            .field("available_frames", &self.available_frames())
            .finish_non_exhaustive()
    }
}

impl PcmReader {
    /// Copies whole frames into `out` and returns how many frames were copied.
    ///
    /// Neither locks nor allocates. Fewer frames than asked for means the
    /// worker has not produced them yet; the caller fills the rest with
    /// silence rather than blocking.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let channels = usize::from(self.channels);
        let wanted = out.len() / channels;
        let ready = self.consumer.occupied_len() / channels;
        let frames = wanted.min(ready);
        if frames == 0 {
            return 0;
        }
        self.consumer.pop_slice(&mut out[..frames * channels]);
        frames
    }

    /// Frames ready to be read right now.
    pub fn available_frames(&self) -> usize {
        self.consumer.occupied_len() / usize::from(self.channels)
    }

    /// Frames the ring holds when empty.
    pub fn capacity_frames(&self) -> usize {
        self.capacity_frames
    }

    /// Channels per frame.
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

/// A [`Resampler`] wired to the writing end of a PCM ring.
///
/// This is the worker-thread half of a source: it takes decoded frames at the
/// source rate, converts them to the sequence rate and hands them to the
/// audio callback through the ring. When the ring is full the converted frames
/// wait in an internal backlog, so nothing is ever dropped; the caller pauses
/// decoding until [`ResampleStage::backlog_frames`] falls back to zero.
#[derive(Debug)]
pub struct ResampleStage {
    /// The per-source converter; its latency is the stage's latency.
    resampler: Resampler,
    /// The ring the audio callback reads.
    writer: PcmWriter,
    /// Converted frames that did not fit in the ring yet.
    backlog: Vec<f32>,
}

impl ResampleStage {
    /// Ties `resampler` to `writer`.
    ///
    /// # Errors
    ///
    /// Returns [`codes::UNSUPPORTED_LAYOUT`] when the two disagree about the
    /// channel count.
    pub fn new(resampler: Resampler, writer: PcmWriter) -> SubResult<Self> {
        if resampler.channels() != writer.channels() {
            return Err(SubError::new(
                codes::UNSUPPORTED_LAYOUT,
                "the resampler and the ring disagree about the channel count",
            )
            .with_detail("resampler_channels", resampler.channels())
            .with_detail("ring_channels", writer.channels()));
        }
        Ok(Self {
            resampler,
            writer,
            backlog: Vec::new(),
        })
    }

    /// The source's fixed latency, at the sequence rate.
    pub fn latency(&self) -> RationalTime {
        self.resampler.latency()
    }

    /// The converter, for its rates and latency in frames.
    pub fn resampler(&self) -> &Resampler {
        &self.resampler
    }

    /// Converted frames still waiting for room in the ring.
    pub fn backlog_frames(&self) -> usize {
        self.backlog.len() / usize::from(self.writer.channels())
    }

    /// Converts `input` and pushes as much as the ring will take, returning
    /// the frames written to the ring.
    ///
    /// # Errors
    ///
    /// The codes of [`Resampler::process`].
    pub fn feed(&mut self, input: &[f32]) -> SubResult<usize> {
        self.resampler.process(input, &mut self.backlog)?;
        Ok(self.drain())
    }

    /// Flushes the filter's tail into the backlog and pushes what fits,
    /// returning the frames written to the ring.
    ///
    /// # Errors
    ///
    /// The codes of [`Resampler::flush`].
    pub fn finish(&mut self) -> SubResult<usize> {
        self.resampler.flush(&mut self.backlog)?;
        Ok(self.drain())
    }

    /// Pushes as much of the backlog into the ring as fits.
    pub fn drain(&mut self) -> usize {
        let frames = self.writer.write(&self.backlog);
        if frames > 0 {
            self.backlog
                .drain(..frames * usize::from(self.writer.channels()));
        }
        frames
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::{PcmReader, ResampleStage, Resampler, pcm_ring};

    /// One second of a mono sine at `freq` hertz sampled at `rate`.
    fn sine(freq: f64, rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|n| (std::f64::consts::TAU * freq * n as f64 / f64::from(rate)).sin() as f32)
            .collect()
    }

    /// Frequency estimated from rising zero crossings, in hertz.
    fn estimate_freq(samples: &[f32], rate: u32) -> f64 {
        let mut first = None;
        let mut last = 0.0;
        let mut crossings = 0u32;
        for (index, window) in samples.windows(2).enumerate() {
            if window[0] <= 0.0 && window[1] > 0.0 {
                // Linear interpolation of the crossing instant, in frames.
                let frac = f64::from(-window[0]) / f64::from(window[1] - window[0]);
                let at = index as f64 + frac;
                if first.is_none() {
                    first = Some(at);
                } else {
                    last = at;
                    crossings += 1;
                }
            }
        }
        let first = first.expect("a tone crosses zero");
        assert!(crossings > 0, "a tone crosses zero more than once");
        f64::from(crossings) * f64::from(rate) / (last - first)
    }

    #[test]
    fn passthrough_when_rates_match() {
        let mut resampler = Resampler::new(48_000, 48_000, 2).expect("build");
        assert!(resampler.is_passthrough());
        assert_eq!(resampler.latency_frames(), 0);
        assert_eq!(resampler.latency().value(), 0);
        let mut out = Vec::new();
        resampler
            .process(&[0.25, -0.25, 0.5, -0.5], &mut out)
            .expect("process");
        resampler.flush(&mut out).expect("flush");
        assert_eq!(out, vec![0.25, -0.25, 0.5, -0.5]);
    }

    #[test]
    fn latency_is_fixed_and_reported_at_the_sequence_rate() {
        let mut resampler = Resampler::new(44_100, 48_000, 1).expect("build");
        let latency = resampler.latency();
        assert!(
            resampler.latency_frames() > 0,
            "a sinc filter delays the signal"
        );
        assert_eq!(
            latency.rate().numerator(),
            48_000,
            "latency is at the sequence rate"
        );
        assert_eq!(
            latency.value(),
            i64::try_from(resampler.latency_frames()).expect("a small latency")
        );

        let mut out = Vec::new();
        for _ in 0..8 {
            resampler
                .process(&sine(1_000.0, 44_100, 4_410), &mut out)
                .expect("process");
            assert_eq!(resampler.latency(), latency, "latency never moves");
        }
        resampler.flush(&mut out).expect("flush");
        assert_eq!(resampler.latency(), latency, "latency never moves");
    }

    #[test]
    fn rejects_bad_shapes() {
        assert!(Resampler::new(0, 48_000, 2).is_err());
        assert!(Resampler::new(44_100, 0, 2).is_err());
        assert!(Resampler::new(44_100, 48_000, 0).is_err());
        assert!(Resampler::new(44_100, 48_000, 4_096).is_err());
        assert!(Resampler::with_chunk_frames(44_100, 48_000, 2, 0).is_err());

        let mut resampler = Resampler::new(44_100, 48_000, 2).expect("build");
        let mut out = Vec::new();
        let err = resampler
            .process(&[0.0; 5], &mut out)
            .expect_err("odd sample count");
        assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
    }

    #[test]
    fn tone_keeps_its_frequency_and_has_no_clicks_at_chunk_boundaries() {
        let source_rate = 44_100;
        let sequence_rate = 48_000;
        let frames = source_rate as usize; // one second
        let input = sine(1_000.0, source_rate, frames);

        // Ragged chunk sizes: every boundary lands somewhere different.
        let mut resampler = Resampler::new(source_rate, sequence_rate, 1).expect("build");
        let mut out = Vec::new();
        let mut offset = 0;
        for size in [17usize, 1, 4_096, 511, 1_024, 3].iter().cycle() {
            if offset >= input.len() {
                break;
            }
            let end = (offset + size).min(input.len());
            resampler
                .process(&input[offset..end], &mut out)
                .expect("process");
            offset = end;
        }
        resampler.flush(&mut out).expect("flush");

        let expected =
            frames * sequence_rate as usize / source_rate as usize + resampler.latency_frames();
        assert_eq!(out.len(), expected, "exact output length");

        // Drop the filter's ramp-in and ramp-out before measuring.
        let body = &out[resampler.latency_frames() + 480..out.len() - 480];

        let freq = estimate_freq(body, sequence_rate);
        assert!(
            (freq - 1_000.0).abs() < 0.5,
            "resampled tone should stay at 1 kHz, measured {freq}"
        );

        let peak = body.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
        assert!(
            (0.98..=1.02).contains(&peak),
            "amplitude should survive, peak {peak}"
        );

        // A click is a step the tone itself cannot make: for a 1 kHz sine at
        // 48 kHz the second difference is bounded by 4 sin^2(pi f / fs) ~= 0.0171.
        let bound = 4.0 * (std::f64::consts::PI * 1_000.0 / 48_000.0).sin().powi(2) * 1.05;
        for (n, w) in body.windows(3).enumerate() {
            let second = f64::from(w[2] - 2.0 * w[1] + w[0]).abs();
            assert!(
                second <= bound,
                "click at output frame {n}: second difference {second} exceeds {bound}"
            );
        }
    }

    #[test]
    fn chunking_does_not_change_the_result() {
        let input = sine(440.0, 44_100, 20_000);

        let mut whole = Resampler::new(44_100, 48_000, 1).expect("build");
        let mut whole_out = Vec::new();
        whole.process(&input, &mut whole_out).expect("process");
        whole.flush(&mut whole_out).expect("flush");

        let mut split = Resampler::new(44_100, 48_000, 1).expect("build");
        let mut split_out = Vec::new();
        for chunk in input.chunks(97) {
            split.process(chunk, &mut split_out).expect("process");
        }
        split.flush(&mut split_out).expect("flush");

        assert_eq!(whole_out, split_out, "chunking must be bit-exact");
    }

    #[test]
    fn stereo_channels_stay_independent() {
        let left = sine(1_000.0, 44_100, 8_820);
        let right = sine(250.0, 44_100, 8_820);
        let mut interleaved = Vec::with_capacity(left.len() * 2);
        for (l, r) in left.iter().zip(&right) {
            interleaved.push(*l);
            interleaved.push(*r);
        }

        let mut resampler = Resampler::new(44_100, 48_000, 2).expect("build");
        let mut out = Vec::new();
        resampler.process(&interleaved, &mut out).expect("process");
        resampler.flush(&mut out).expect("flush");

        let skip = resampler.latency_frames() + 480;
        let body = &out[skip * 2..out.len() - 960];
        let left_out: Vec<f32> = body.iter().step_by(2).copied().collect();
        let right_out: Vec<f32> = body.iter().skip(1).step_by(2).copied().collect();
        assert!((estimate_freq(&left_out, 48_000) - 1_000.0).abs() < 0.5);
        assert!((estimate_freq(&right_out, 48_000) - 250.0).abs() < 0.5);
    }

    #[test]
    fn ring_moves_whole_frames_only() {
        let (mut writer, mut reader) = pcm_ring(2, 4).expect("ring");
        assert_eq!(writer.vacant_frames(), 4);
        assert_eq!(writer.write(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 3);
        assert_eq!(reader.available_frames(), 3);
        let mut out = [0.0f32; 4];
        assert_eq!(reader.read(&mut out), 2);
        assert_eq!(out.to_vec(), vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(reader.available_frames(), 1);
        // A buffer too small for a whole frame takes nothing.
        let mut tiny = [0.0f32; 1];
        assert_eq!(reader.read(&mut tiny), 0);
    }

    #[test]
    fn stage_runs_off_the_audio_thread_and_feeds_the_ring() {
        let source_rate = 44_100;
        let sequence_rate = 48_000;
        let frames = 44_100;
        let input = sine(1_000.0, source_rate, frames);
        let expected = frames * sequence_rate as usize / source_rate as usize;

        let resampler = Resampler::new(source_rate, sequence_rate, 1).expect("build");
        let latency = resampler.latency_frames();
        let (writer, mut reader) = pcm_ring(1, 2_048).expect("ring");
        let mut stage = ResampleStage::new(resampler, writer).expect("stage");

        // The worker thread: decode-side work, allowed to allocate and block.
        let worker = std::thread::spawn(move || -> sub_core::SubResult<()> {
            for chunk in input.chunks(512) {
                stage.feed(chunk)?;
                while stage.backlog_frames() > 0 {
                    if stage.drain() == 0 {
                        std::thread::yield_now();
                    }
                }
            }
            stage.finish()?;
            while stage.backlog_frames() > 0 {
                if stage.drain() == 0 {
                    std::thread::yield_now();
                }
            }
            Ok(())
        });

        // The callback side: fixed-size reads into a pre-allocated block.
        let mut block = [0.0f32; 256];
        let mut collected = Vec::with_capacity(expected + latency);
        while collected.len() < expected + latency {
            let got = read_no_alloc(&mut reader, &mut block);
            if got == 0 {
                std::thread::yield_now();
                continue;
            }
            collected.extend_from_slice(&block[..got]);
        }
        worker
            .join()
            .expect("worker thread")
            .expect("worker result");

        assert_eq!(collected.len(), expected + latency);
        let body = &collected[latency + 480..collected.len() - 480];
        assert!((estimate_freq(body, sequence_rate) - 1_000.0).abs() < 0.5);
    }

    /// The audio-callback side: a plain copy, no lock and no allocation.
    fn read_no_alloc(reader: &mut PcmReader, block: &mut [f32]) -> usize {
        reader.read(block)
    }
}
