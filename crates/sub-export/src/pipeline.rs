//! The export pipeline: composited frames and mixed audio into a file
//! (docs/PLAN.md §5.5).
//!
//! Two `appsrc` elements feed one pipeline. The video one takes the tightly
//! packed RGBA frames the compositor reads back, converts them and hands them
//! to the encoder the capability probe chose; the audio one takes interleaved
//! `f32` frames from the offline mixer render. Both land in the muxer the
//! container asks for, and the muxer writes the file.
//!
//! Every timestamp is computed from the sequence's [`Rational`] frame rate and
//! the sample rate with integer arithmetic, never from a float: frame `i`
//! starts at `i * den / num` seconds exactly, and the audio frames that belong
//! to video frame `i` are the half-open span
//! `[audio_frames_through(i), audio_frames_through(i + 1))`. The two clocks
//! therefore never drift apart, however long the export runs and whatever the
//! rate — 23.976 fps at 48 kHz included.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use std::path::Path;
//! use sub_export::{Container, ExportSettings, PcmAudioSource, SolidFrames, export};
//! use sub_time::Rational;
//!
//! let settings = ExportSettings::new(1920, 1080, Rational::FPS_24, Container::Mp4);
//! // In a real export the frames come from the compositor's readback path.
//! let mut frames = SolidFrames::new(settings.frame_bytes(), 24);
//! let mut audio = PcmAudioSource::new(vec![0.0; 48_000 * 2]);
//! let report = export(Path::new("out.mp4"), &settings, &mut frames, Some(&mut audio))?;
//! println!("wrote {} frames", report.video_frames);
//! # Ok(())
//! # }
//! ```

use std::path::{Path, PathBuf};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSrc;
use serde::{Deserialize, Serialize};
use sub_core::{ErrorCode, SubError, SubResult};
use sub_time::{Rational, RationalTime};

use crate::codes;
use crate::encoder::{EncoderPreferences, EncoderProbe, VideoCodec, element_is_usable};

/// Nanoseconds in one second, the unit GStreamer timestamps use.
const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// Bytes of RGBA per pixel, matching `sub_render::readback::BYTES_PER_PIXEL`.
pub const BYTES_PER_PIXEL: usize = 4;

/// How many bytes each `appsrc` queues before a push blocks.
///
/// The driver pushes video and audio in lockstep, so neither branch runs far
/// ahead of the other and the cap only bounds the encoder's backlog.
const APPSRC_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// How long [`ExportPipeline::finish`] waits for the muxer before giving up.
const EOS_TIMEOUT_SECONDS: u64 = 120;

/// The file format the export is wrapped in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    /// MPEG-4 part 14, written by `mp4mux`.
    Mp4,
    /// Matroska, written by `matroskamux`.
    Mkv,
    /// `QuickTime`, written by `qtmux`.
    Mov,
}

/// Every container, in the order they are reported.
pub const CONTAINERS: [Container; 3] = [Container::Mp4, Container::Mkv, Container::Mov];

impl Container {
    /// The stable identifier used in presets, settings and agent calls.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Mov => "mov",
        }
    }

    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        self.as_str()
    }

    /// The GStreamer muxer element that writes it.
    pub fn muxer(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4mux",
            Self::Mkv => "matroskamux",
            Self::Mov => "qtmux",
        }
    }

    /// Parses the stable identifier.
    pub fn parse(name: &str) -> Option<Self> {
        CONTAINERS
            .into_iter()
            .find(|container| container.as_str() == name)
    }

    /// Whether this container can carry `codec`.
    ///
    /// Matroska carries everything the exporter produces; the ISO base media
    /// formats carry what their specifications name, and AV1 in `QuickTime`
    /// is not something `qtmux` writes.
    pub fn accepts_video(self, codec: VideoCodec) -> bool {
        match self {
            Self::Mkv | Self::Mp4 => true,
            Self::Mov => codec != VideoCodec::Av1,
        }
    }

    /// Whether this container can carry `codec`.
    pub fn accepts_audio(self, codec: AudioCodec) -> bool {
        match self {
            Self::Mkv => true,
            Self::Mp4 | Self::Mov => codec == AudioCodec::Aac,
        }
    }
}

impl std::fmt::Display for Container {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An audio codec an export can target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    /// AAC-LC, what the ISO containers carry.
    Aac,
    /// Opus, for Matroska.
    Opus,
    /// FLAC: lossless, for a master or a sync check.
    Flac,
}

/// Every audio codec, in the order they are reported.
pub const AUDIO_CODECS: [AudioCodec; 3] = [AudioCodec::Aac, AudioCodec::Opus, AudioCodec::Flac];

impl AudioCodec {
    /// The stable identifier used in presets, settings and agent calls.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Opus => "opus",
            Self::Flac => "flac",
        }
    }

    /// A human-readable name for the panel.
    pub fn label(self) -> &'static str {
        match self {
            Self::Aac => "AAC",
            Self::Opus => "Opus",
            Self::Flac => "FLAC",
        }
    }

    /// Parses the stable identifier.
    pub fn parse(name: &str) -> Option<Self> {
        AUDIO_CODECS
            .into_iter()
            .find(|codec| codec.as_str() == name)
    }

    /// The encoder elements that produce it, best first.
    ///
    /// A stock GStreamer install carries more than one AAC encoder, so the
    /// list is an order exactly as the video catalogue is: the first usable
    /// one wins.
    pub fn encoders(self) -> &'static [&'static str] {
        match self {
            Self::Aac => &["avenc_aac", "voaacenc", "fdkaacenc", "faac"],
            Self::Opus => &["opusenc"],
            Self::Flac => &["flacenc"],
        }
    }

    /// The parser that normalises the encoder's output for the muxer, when the
    /// codec has one.
    pub fn parser(self) -> Option<&'static str> {
        match self {
            Self::Aac => Some("aacparse"),
            Self::Opus => Some("opusparse"),
            Self::Flac => None,
        }
    }
}

impl std::fmt::Display for AudioCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The parser that normalises a video encoder's output for the muxer.
///
/// Every video codec has one; whether this machine carries the element is a
/// separate question, answered by [`element_is_usable`].
fn video_parser(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "h264parse",
        VideoCodec::H265 => "h265parse",
        VideoCodec::Av1 => "av1parse",
    }
}

/// What an export writes: the canvas, the rates and the codecs.
///
/// The audio side is optional: an export with no audio builds a video-only
/// pipeline, which is what a sequence with no audio tracks wants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportSettings {
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// The sequence frame rate, exactly as the sequence states it.
    pub frame_rate: Rational,
    /// The container the streams are wrapped in.
    pub container: Container,
    /// The video codec to encode.
    pub video_codec: VideoCodec,
    /// The audio codec to encode, or `None` for a video-only export.
    pub audio_codec: Option<AudioCodec>,
    /// The sample rate of the audio handed to the pipeline, in hertz.
    pub sample_rate: u32,
    /// Channels per audio frame.
    pub channels: u16,
}

impl ExportSettings {
    /// Settings for `width` by `height` at `frame_rate` in `container`, with
    /// H.264 video and 48 kHz stereo AAC audio.
    pub fn new(width: u32, height: u32, frame_rate: Rational, container: Container) -> Self {
        Self {
            width,
            height,
            frame_rate,
            container,
            video_codec: VideoCodec::H264,
            audio_codec: Some(AudioCodec::Aac),
            sample_rate: 48_000,
            channels: 2,
        }
    }

    /// The same settings with `codec` as the video codec.
    #[must_use]
    pub fn with_video_codec(mut self, codec: VideoCodec) -> Self {
        self.video_codec = codec;
        self
    }

    /// The same settings with `codec` as the audio codec, or no audio at all.
    #[must_use]
    pub fn with_audio_codec(mut self, codec: Option<AudioCodec>) -> Self {
        self.audio_codec = codec;
        self
    }

    /// The same settings with `rate` hertz and `channels` audio channels.
    #[must_use]
    pub fn with_audio_format(mut self, rate: u32, channels: u16) -> Self {
        self.sample_rate = rate;
        self.channels = channels;
        self
    }

    /// Bytes in one RGBA frame of this canvas.
    pub fn frame_bytes(&self) -> usize {
        self.width as usize * self.height as usize * BYTES_PER_PIXEL
    }

    /// Checks the settings describe a file the exporter can actually write.
    ///
    /// # Errors
    ///
    /// - [`codes::INVALID_SETTINGS`] for a zero dimension, sample rate or
    ///   channel count.
    /// - [`codes::UNSUPPORTED_COMBINATION`] when the container cannot carry
    ///   one of the codecs.
    pub fn validate(&self) -> SubResult<()> {
        if self.width == 0 || self.height == 0 {
            return Err(SubError::new(
                codes::INVALID_SETTINGS,
                "an export canvas needs a non-zero width and height",
            )
            .with_detail("width", self.width)
            .with_detail("height", self.height));
        }
        if !self.container.accepts_video(self.video_codec) {
            return Err(SubError::new(
                codes::UNSUPPORTED_COMBINATION,
                format!(
                    "{} cannot carry {}",
                    self.container,
                    self.video_codec.label()
                ),
            )
            .with_detail("container", self.container.as_str())
            .with_detail("codec", self.video_codec.as_str()));
        }
        if let Some(audio) = self.audio_codec {
            if self.sample_rate == 0 || self.channels == 0 {
                return Err(SubError::new(
                    codes::INVALID_SETTINGS,
                    "an audio export needs a non-zero sample rate and channel count",
                )
                .with_detail("sample_rate", self.sample_rate)
                .with_detail("channels", self.channels));
            }
            if !self.container.accepts_audio(audio) {
                return Err(SubError::new(
                    codes::UNSUPPORTED_COMBINATION,
                    format!("{} cannot carry {}", self.container, audio.label()),
                )
                .with_detail("container", self.container.as_str())
                .with_detail("audio_codec", audio.as_str()));
            }
        }
        Ok(())
    }

    /// The start of video frame `index`, in nanoseconds.
    ///
    /// Exact integer arithmetic on the [`Rational`] rate: no float ever sees a
    /// timestamp, so the 86400th frame of a 23.976 fps export sits where the
    /// sequence says it does.
    pub fn frame_start_nanos(&self, index: u64) -> u64 {
        let num = u128::from(self.frame_rate.numerator());
        let den = u128::from(self.frame_rate.denominator());
        let nanos = u128::from(index) * den * NANOS_PER_SECOND / num;
        u64::try_from(nanos).unwrap_or(u64::MAX)
    }

    /// How many audio frames belong to the whole of the first `frames` video
    /// frames.
    ///
    /// The difference between two consecutive values is the audio one video
    /// frame owns, which is what keeps the two clocks in lockstep at a rate
    /// like 29.97 fps where no single frame holds a whole number of samples.
    pub fn audio_frames_through(&self, frames: u64) -> u64 {
        let num = u128::from(self.frame_rate.numerator());
        let den = u128::from(self.frame_rate.denominator());
        let total = u128::from(frames) * den * u128::from(self.sample_rate) / num;
        u64::try_from(total).unwrap_or(u64::MAX)
    }

    /// The duration of `frames` video frames, at the sequence rate.
    pub fn duration(&self, frames: u64) -> RationalTime {
        RationalTime::from_frames(i64::try_from(frames).unwrap_or(i64::MAX), self.frame_rate)
    }
}

/// A source of composited frames, handed to the exporter one frame at a time.
///
/// Frames are tightly packed RGBA rows, top row first: exactly what
/// `sub_render::readback::FrameBuffer::pixels` returns.
pub trait VideoFrameSource {
    /// The next frame, or `None` when the sequence is over.
    ///
    /// # Errors
    ///
    /// Whatever [`SubError`] the compositor raises while rendering.
    fn next_frame(&mut self) -> SubResult<Option<&[u8]>>;
}

/// A source of mixed audio, read in the spans the video frames ask for.
///
/// This is the offline render's side of the export: the pipeline pulls exactly
/// the frames that belong to the video frame it just pushed, so the mix never
/// runs ahead of or behind the picture.
pub trait AudioFrameSource {
    /// Fills `out` with interleaved frames at `channels` channels and returns
    /// how many whole frames were written.
    ///
    /// Fewer than asked for, zero included, means the mix is over; the
    /// exporter pads the rest of the export with silence so the audio and
    /// video streams end together.
    ///
    /// # Errors
    ///
    /// Whatever [`SubError`] the mixer raises while rendering.
    fn read(&mut self, out: &mut [f32], channels: u16) -> SubResult<usize>;
}

/// An [`AudioFrameSource`] over interleaved frames already rendered into
/// memory, which is the shape `sub_audio::render_audio` returns.
#[derive(Debug, Clone, Default)]
pub struct PcmAudioSource {
    samples: Vec<f32>,
    read: usize,
}

impl PcmAudioSource {
    /// A source over `samples`, interleaved at the export's channel count.
    pub fn new(samples: Vec<f32>) -> Self {
        Self { samples, read: 0 }
    }

    /// How many samples are left to hand out.
    pub fn remaining(&self) -> usize {
        self.samples.len() - self.read
    }
}

impl AudioFrameSource for PcmAudioSource {
    fn read(&mut self, out: &mut [f32], channels: u16) -> SubResult<usize> {
        let channels = usize::from(channels.max(1));
        let wanted = (out.len() / channels) * channels;
        let available = self.remaining() / channels * channels;
        let taken = wanted.min(available);
        out[..taken].copy_from_slice(&self.samples[self.read..self.read + taken]);
        self.read += taken;
        Ok(taken / channels)
    }
}

/// A [`VideoFrameSource`] that hands out one frame of black a fixed number of
/// times, for doctests and smoke tests.
#[derive(Debug, Clone)]
pub struct SolidFrames {
    pixels: Vec<u8>,
    left: u64,
}

impl SolidFrames {
    /// `count` frames of `bytes` opaque black pixels.
    pub fn new(bytes: usize, count: u64) -> Self {
        let mut pixels = vec![0; bytes];
        for alpha in pixels.iter_mut().skip(3).step_by(BYTES_PER_PIXEL) {
            *alpha = 0xff;
        }
        Self {
            pixels,
            left: count,
        }
    }
}

impl VideoFrameSource for SolidFrames {
    fn next_frame(&mut self) -> SubResult<Option<&[u8]>> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        Ok(Some(&self.pixels))
    }
}

/// What an export produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportReport {
    /// The file that was written.
    pub path: PathBuf,
    /// Video frames pushed into the encoder.
    pub video_frames: u64,
    /// Audio frames pushed into the encoder, silence padding included.
    pub audio_frames: u64,
    /// The exported duration, at the sequence frame rate.
    pub duration: RationalTime,
    /// The video encoder element that ran.
    pub video_encoder: String,
    /// The audio encoder element that ran, when there was audio.
    pub audio_encoder: Option<String>,
    /// The muxer element that wrote the file.
    pub muxer: String,
}

/// The elements one export runs, resolved before the pipeline is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportElements {
    /// The video encoder element.
    pub video_encoder: String,
    /// The audio encoder element, when the export has audio.
    pub audio_encoder: Option<String>,
}

impl ExportElements {
    /// Resolves the elements for `settings` from the session's encoder probe
    /// and the user's overrides.
    ///
    /// # Errors
    ///
    /// The validation errors of [`ExportSettings::validate`], the selection
    /// errors of [`EncoderProbe::select`], and [`codes::NO_ENCODER`] when
    /// nothing on this machine encodes the audio codec.
    pub fn resolve(settings: &ExportSettings, preferences: &EncoderPreferences) -> SubResult<Self> {
        settings.validate()?;
        let probe = EncoderProbe::cached()?;
        let video = probe.select(settings.video_codec, preferences)?;
        let audio = settings
            .audio_codec
            .map(select_audio_encoder)
            .transpose()?
            .map(str::to_owned);
        Ok(Self {
            video_encoder: video.element.clone(),
            audio_encoder: audio,
        })
    }
}

/// The first usable encoder for `codec`.
///
/// # Errors
///
/// [`codes::NO_ENCODER`] when none of the codec's encoders is usable here.
fn select_audio_encoder(codec: AudioCodec) -> SubResult<&'static str> {
    codec
        .encoders()
        .iter()
        .copied()
        .find(|name| element_is_usable(name))
        .ok_or_else(|| {
            SubError::new(
                codes::NO_ENCODER,
                format!("no {codec} encoder is available on this machine"),
            )
            .with_detail("audio_codec", codec.as_str())
            .with_detail("known", codec.encoders())
        })
}

/// A built export pipeline, waiting to be fed.
///
/// The caller pushes video frames and the audio that belongs to them, then
/// calls [`ExportPipeline::finish`]. [`export`] does that in lockstep and is
/// what most callers want; the pieces are public so an export job can drive
/// the loop itself, report progress and cancel between frames.
pub struct ExportPipeline {
    pipeline: gst::Pipeline,
    video_src: AppSrc,
    audio_src: Option<AppSrc>,
    settings: ExportSettings,
    elements: ExportElements,
    path: PathBuf,
    video_frames: u64,
    audio_frames: u64,
    finished: bool,
}

impl std::fmt::Debug for ExportPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportPipeline")
            .field("path", &self.path)
            .field("video_encoder", &self.elements.video_encoder)
            .field("audio_encoder", &self.elements.audio_encoder)
            .field("video_frames", &self.video_frames)
            .field("audio_frames", &self.audio_frames)
            .field("has_audio", &self.audio_src.is_some())
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl ExportPipeline {
    /// Builds the pipeline that writes `path` and starts it.
    ///
    /// # Errors
    ///
    /// - [`codes::INIT_FAILED`] when GStreamer will not start.
    /// - The validation errors of [`ExportSettings::validate`].
    /// - [`codes::MUXER_UNAVAILABLE`] when the container's muxer is missing,
    ///   and [`codes::ENCODER_UNAVAILABLE`] when a chosen encoder is.
    /// - [`codes::PIPELINE_FAILED`] when an element cannot be built, linked or
    ///   started.
    pub fn new(
        path: &Path,
        settings: &ExportSettings,
        elements: &ExportElements,
    ) -> SubResult<Self> {
        settings.validate()?;
        gst::init()
            .map_err(|e| SubError::wrap(codes::INIT_FAILED, "GStreamer failed to start", &e))?;
        let pipeline = gst::Pipeline::new();
        let muxer = make_coded_element(settings.container.muxer(), codes::MUXER_UNAVAILABLE)
            .map_err(|e| e.with_detail("container", settings.container.as_str()))?;
        let sink = make_element("filesink")?;
        sink.set_property("location", path.to_string_lossy().as_ref());
        sink.set_property("sync", false);
        pipeline
            .add_many([&muxer, &sink])
            .map_err(|e| pipeline_error("the muxer could not be added", &e))?;
        muxer
            .link(&sink)
            .map_err(|e| pipeline_error("the muxer would not link to the file", &e))?;

        let video_src = build_video_branch(&pipeline, &muxer, settings, elements)?;
        let audio_src = match (settings.audio_codec, elements.audio_encoder.as_deref()) {
            (Some(codec), Some(encoder)) => Some(build_audio_branch(
                &pipeline, &muxer, settings, codec, encoder,
            )?),
            _ => None,
        };

        pipeline.set_state(gst::State::Playing).map_err(|e| {
            let error = pipeline_error("the export pipeline would not start", &e)
                .with_detail("path", path.display().to_string());
            // The state change error itself says nothing about which element
            // refused; the bus does, so it is drained before the pipeline is
            // torn down.
            let error = with_bus_error(&pipeline, error);
            let _ = pipeline.set_state(gst::State::Null);
            error
        })?;

        tracing::debug!(
            path = %path.display(),
            video_encoder = elements.video_encoder,
            audio_encoder = elements.audio_encoder,
            muxer = settings.container.muxer(),
            "export pipeline started"
        );
        Ok(Self {
            pipeline,
            video_src,
            audio_src,
            settings: settings.clone(),
            elements: elements.clone(),
            path: path.to_owned(),
            video_frames: 0,
            audio_frames: 0,
            finished: false,
        })
    }

    /// The settings the pipeline was built for.
    pub fn settings(&self) -> &ExportSettings {
        &self.settings
    }

    /// The file being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The encoder elements the pipeline is running.
    pub fn elements(&self) -> &ExportElements {
        &self.elements
    }

    /// How many bytes the muxer has written to the file so far.
    ///
    /// A muxer that buffers its header reports zero for a while, which is why
    /// this is a progress statistic and never a completion test.
    pub fn bytes_written(&self) -> u64 {
        std::fs::metadata(&self.path).map_or(0, |meta| meta.len())
    }

    /// Video frames pushed so far.
    pub fn video_frames(&self) -> u64 {
        self.video_frames
    }

    /// Audio frames pushed so far.
    pub fn audio_frames(&self) -> u64 {
        self.audio_frames
    }

    /// Whether the pipeline carries an audio stream.
    pub fn has_audio(&self) -> bool {
        self.audio_src.is_some()
    }

    /// How many audio frames belong to the video frame that comes next.
    pub fn audio_frames_for_next_video_frame(&self) -> u64 {
        self.settings
            .audio_frames_through(self.video_frames + 1)
            .saturating_sub(self.settings.audio_frames_through(self.video_frames))
    }

    /// Pushes one tightly packed RGBA frame, timestamped at the frame it is.
    ///
    /// # Errors
    ///
    /// - [`codes::INVALID_SETTINGS`] when `pixels` is not one whole frame of
    ///   the export canvas.
    /// - [`codes::PUSH_FAILED`] when the pipeline will not take it, which is
    ///   how a failing encoder surfaces mid-export.
    pub fn push_video_frame(&mut self, pixels: &[u8]) -> SubResult<()> {
        let expected = self.settings.frame_bytes();
        if pixels.len() != expected {
            return Err(SubError::new(
                codes::INVALID_SETTINGS,
                "a pushed frame is not the size of the export canvas",
            )
            .with_detail("expected_bytes", expected)
            .with_detail("actual_bytes", pixels.len()));
        }
        let index = self.video_frames;
        let start = self.settings.frame_start_nanos(index);
        let end = self.settings.frame_start_nanos(index + 1);
        let buffer = timed_buffer(
            pixels.to_vec(),
            start,
            end.saturating_sub(start),
            index,
            index + 1,
        )?;
        self.video_src
            .push_buffer(buffer)
            .map_err(|flow| self.push_failed("video", flow))?;
        self.video_frames += 1;
        Ok(())
    }

    /// Pushes interleaved audio frames, timestamped at the frames they are.
    ///
    /// # Errors
    ///
    /// - [`codes::INVALID_SETTINGS`] when the export has no audio stream or
    ///   `samples` is not a whole number of interleaved frames.
    /// - [`codes::PUSH_FAILED`] when the pipeline will not take it.
    pub fn push_audio(&mut self, samples: &[f32]) -> SubResult<()> {
        let channels = usize::from(self.settings.channels);
        let Some(src) = self.audio_src.as_ref() else {
            return Err(SubError::new(
                codes::INVALID_SETTINGS,
                "this export has no audio stream to push into",
            ));
        };
        if channels == 0 || !samples.len().is_multiple_of(channels) {
            return Err(SubError::new(
                codes::INVALID_SETTINGS,
                "pushed audio is not a whole number of interleaved frames",
            )
            .with_detail("samples", samples.len())
            .with_detail("channels", channels));
        }
        if samples.is_empty() {
            return Ok(());
        }
        let frames = (samples.len() / channels) as u64;
        let start = audio_nanos(self.audio_frames, self.settings.sample_rate);
        let end = audio_nanos(self.audio_frames + frames, self.settings.sample_rate);
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let buffer = timed_buffer(
            bytes,
            start,
            end.saturating_sub(start),
            self.audio_frames,
            self.audio_frames + frames,
        )?;
        src.push_buffer(buffer)
            .map_err(|flow| self.push_failed("audio", flow))?;
        self.audio_frames += frames;
        Ok(())
    }

    /// Ends both streams, waits for the muxer to write the file and reports
    /// what was written.
    ///
    /// # Errors
    ///
    /// [`codes::PIPELINE_FAILED`] when the pipeline posted an error, and
    /// [`codes::EXPORT_TIMEOUT`] when it never reached end of stream.
    pub fn finish(mut self) -> SubResult<ExportReport> {
        self.finished = true;
        let report = self.finish_streams();
        // Whether the muxer finished or failed, the pipeline stops here: a
        // failed export must not leave elements running behind the error.
        let _ = self.pipeline.set_state(gst::State::Null);
        report
    }

    /// Ends both streams and waits for the muxer, leaving the pipeline state
    /// for [`ExportPipeline::finish`] to tear down either way.
    fn finish_streams(&self) -> SubResult<ExportReport> {
        end_stream(&self.video_src)?;
        if let Some(src) = self.audio_src.as_ref() {
            end_stream(src)?;
        }
        self.wait_for_eos()?;
        Ok(ExportReport {
            path: self.path.clone(),
            video_frames: self.video_frames,
            audio_frames: self.audio_frames,
            duration: self.settings.duration(self.video_frames),
            video_encoder: self.elements.video_encoder.clone(),
            audio_encoder: self.elements.audio_encoder.clone(),
            muxer: self.settings.container.muxer().to_owned(),
        })
    }

    /// Stops the pipeline without finishing the file. The part-written file is
    /// left for the caller to delete; [`ExportPipeline::abort`] is the usual
    /// choice, because a part-written file is never playable.
    pub fn cancel(mut self) {
        self.finished = true;
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    /// Stops the pipeline and deletes the part-written file, which is what a
    /// cancelled or failed export does.
    ///
    /// A muxer that never saw end of stream has written no index and, for the
    /// ISO containers, no `moov` atom at all: what is on disk is a broken file
    /// wearing a real name, so it goes. Reports whether a file was removed.
    pub fn abort(self) -> bool {
        let path = self.path.clone();
        self.cancel();
        remove_partial_file(&path)
    }

    /// Waits for end of stream, failing on the first error the bus carries.
    fn wait_for_eos(&self) -> SubResult<()> {
        let Some(bus) = self.pipeline.bus() else {
            return Err(SubError::new(
                codes::PIPELINE_FAILED,
                "the export pipeline has no bus",
            ));
        };
        let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(EOS_TIMEOUT_SECONDS),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        ) else {
            return Err(SubError::new(
                codes::EXPORT_TIMEOUT,
                "the export pipeline never finished writing",
            )
            .with_detail("path", self.path.display().to_string())
            .with_detail("timeout_seconds", EOS_TIMEOUT_SECONDS));
        };
        match message.view() {
            gst::MessageView::Eos(_) => Ok(()),
            gst::MessageView::Error(err) => {
                Err(
                    element_error(codes::PIPELINE_FAILED, "the export pipeline failed", err)
                        .with_detail("path", self.path.display().to_string()),
                )
            }
            _ => Err(SubError::new(
                codes::PIPELINE_FAILED,
                "the export pipeline posted something other than end of stream",
            )),
        }
    }

    /// The error a refused push turns into, enriched with whatever the bus
    /// says went wrong underneath.
    fn push_failed(&self, stream: &'static str, flow: gst::FlowError) -> SubError {
        let error = SubError::new(
            codes::PUSH_FAILED,
            format!("the export pipeline refused a {stream} buffer"),
        )
        .with_detail("stream", stream)
        .with_detail("flow", format!("{flow:?}"))
        .with_detail("path", self.path.display().to_string());
        with_bus_error(&self.pipeline, error)
    }
}

/// Deletes a part-written export, reporting whether anything was there.
///
/// A file that was never created is not a problem; anything else is worth a
/// line in the log, because it means a broken file survived a cancellation.
pub(crate) fn remove_partial_file(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "a part-written export could not be removed"
                );
            }
            false
        }
    }
}

/// An error naming the element that posted `err`.
///
/// The element name is what makes a GStreamer failure actionable: `x264enc`
/// refusing to negotiate and `mp4mux` refusing a stream carry much the same
/// error text otherwise. It goes into the message as well as the details, so
/// it survives being rendered as one line in a log or a panel.
fn element_error(code: ErrorCode, message: &str, err: &gst::message::Error) -> SubError {
    let element = err.src().map(|src| src.name().to_string());
    let path = err.src().map(|src| src.path_string().to_string());
    let message = match &element {
        Some(name) => format!("{message} in {name}"),
        None => message.to_owned(),
    };
    SubError::wrap(code, message, &err.error())
        .with_detail("element", element)
        .with_detail("element_path", path)
        .with_detail("reason", err.error().to_string())
        .with_detail("debug", err.debug().map(|debug| debug.to_string()))
}

/// Adds the first error on `pipeline`'s bus to `error`, when there is one.
///
/// A refused push and a refused state change both say only that something went
/// wrong; the element that actually failed posted its reason to the bus.
fn with_bus_error(pipeline: &gst::Pipeline, error: SubError) -> SubError {
    let Some(bus) = pipeline.bus() else {
        return error;
    };
    while let Some(message) = bus.pop() {
        if let gst::MessageView::Error(err) = message.view() {
            let mut merged = element_error(error.code.clone(), &error.message, err);
            for (key, value) in error.details {
                merged.details.entry(key).or_insert(value);
            }
            return merged;
        }
    }
    error
}

impl Drop for ExportPipeline {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.pipeline.set_state(gst::State::Null);
        }
    }
}

/// One buffer carrying `data`, stamped at `start` for `duration` nanoseconds
/// and covering the half-open offset span `[offset, offset_end)`.
fn timed_buffer(
    data: Vec<u8>,
    start: u64,
    duration: u64,
    offset: u64,
    offset_end: u64,
) -> SubResult<gst::Buffer> {
    let mut buffer = gst::Buffer::from_mut_slice(data);
    {
        let buffer = buffer.get_mut().ok_or_else(|| {
            SubError::new(codes::PUSH_FAILED, "a fresh buffer was already shared")
        })?;
        buffer.set_pts(gst::ClockTime::from_nseconds(start));
        buffer.set_dts(gst::ClockTime::from_nseconds(start));
        buffer.set_duration(gst::ClockTime::from_nseconds(duration));
        buffer.set_offset(offset);
        buffer.set_offset_end(offset_end);
    }
    Ok(buffer)
}

/// The nanosecond timestamp of audio frame `frames` at `rate` hertz.
fn audio_nanos(frames: u64, rate: u32) -> u64 {
    let rate = u128::from(rate.max(1));
    let nanos = u128::from(frames) * NANOS_PER_SECOND / rate;
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

/// Runs a whole export, pushing the mix in lockstep with the picture.
///
/// One video frame goes in, then exactly the audio frames that belong to it,
/// so the muxer sees both streams advancing together and neither branch queues
/// much more than the other. When the mix runs out before the picture does the
/// rest is padded with silence, which is what makes the two streams end on the
/// same timestamp.
///
/// # Errors
///
/// The errors of [`ExportElements::resolve`], [`ExportPipeline::new`],
/// [`ExportPipeline::push_video_frame`] and [`ExportPipeline::finish`], plus
/// whatever the sources raise.
pub fn export(
    path: &Path,
    settings: &ExportSettings,
    video: &mut dyn VideoFrameSource,
    audio: Option<&mut dyn AudioFrameSource>,
) -> SubResult<ExportReport> {
    let elements = ExportElements::resolve(settings, &EncoderPreferences::new())?;
    export_with(path, settings, &elements, video, audio, &mut |_| {})
}

/// [`export`] with the elements already resolved and a progress callback
/// called after every frame with the number of frames written so far.
///
/// # Errors
///
/// As [`export`].
pub fn export_with(
    path: &Path,
    settings: &ExportSettings,
    elements: &ExportElements,
    video: &mut dyn VideoFrameSource,
    mut audio: Option<&mut dyn AudioFrameSource>,
    progress: &mut dyn FnMut(u64),
) -> SubResult<ExportReport> {
    let mut pipeline = ExportPipeline::new(path, settings, elements)?;
    let channels = usize::from(settings.channels.max(1));
    let mut block: Vec<f32> = Vec::new();
    while let Some(pixels) = video.next_frame()? {
        let wanted = pipeline.audio_frames_for_next_video_frame();
        pipeline.push_video_frame(pixels)?;
        if pipeline.has_audio() {
            let samples = usize::try_from(wanted).unwrap_or(usize::MAX) * channels;
            block.clear();
            block.resize(samples, 0.0);
            if let Some(source) = audio.as_deref_mut() {
                let filled = source.read(&mut block, settings.channels)?;
                // Silence past the end of the mix keeps the streams the same
                // length, so the file ends on one timestamp.
                for sample in &mut block[filled * channels..] {
                    *sample = 0.0;
                }
            }
            pipeline.push_audio(&block)?;
        }
        progress(pipeline.video_frames());
    }
    pipeline.finish()
}

/// Builds `appsrc ! videoconvert ! encoder ! parser ! muxer`.
fn build_video_branch(
    pipeline: &gst::Pipeline,
    muxer: &gst::Element,
    settings: &ExportSettings,
    elements: &ExportElements,
) -> SubResult<AppSrc> {
    let src = make_element("appsrc")?;
    let convert = make_element("videoconvert")?;
    let encoder = make_coded_element(&elements.video_encoder, codes::ENCODER_UNAVAILABLE)?;
    let mut chain = vec![src.clone(), convert, encoder];
    let parser = video_parser(settings.video_codec);
    if element_is_usable(parser) {
        chain.push(make_element(parser)?);
    }
    link_branch(pipeline, muxer, &chain, "video")?;

    let src = src
        .downcast::<AppSrc>()
        .map_err(|_| SubError::new(codes::PIPELINE_FAILED, "appsrc has the wrong type"))?;
    src.set_caps(Some(&video_caps(settings)));
    configure_appsrc(&src);
    Ok(src)
}

/// Builds `appsrc ! audioconvert ! audioresample ! encoder ! parser ! muxer`.
fn build_audio_branch(
    pipeline: &gst::Pipeline,
    muxer: &gst::Element,
    settings: &ExportSettings,
    codec: AudioCodec,
    encoder_name: &str,
) -> SubResult<AppSrc> {
    let src = make_element("appsrc")?;
    let convert = make_element("audioconvert")?;
    let resample = make_element("audioresample")?;
    let encoder = make_coded_element(encoder_name, codes::ENCODER_UNAVAILABLE)?;
    let mut chain = vec![src.clone(), convert, resample, encoder];
    if let Some(parser) = codec.parser().filter(|name| element_is_usable(name)) {
        chain.push(make_element(parser)?);
    }
    link_branch(pipeline, muxer, &chain, "audio")?;

    let src = src
        .downcast::<AppSrc>()
        .map_err(|_| SubError::new(codes::PIPELINE_FAILED, "appsrc has the wrong type"))?;
    src.set_caps(Some(&audio_caps(settings)));
    configure_appsrc(&src);
    Ok(src)
}

/// Adds one branch to the pipeline, links it end to end and links its tail to
/// the muxer.
fn link_branch(
    pipeline: &gst::Pipeline,
    muxer: &gst::Element,
    chain: &[gst::Element],
    stream: &str,
) -> SubResult<()> {
    let refs: Vec<&gst::Element> = chain.iter().collect();
    pipeline
        .add_many(refs.as_slice())
        .map_err(|e| pipeline_error(&format!("the {stream} branch could not be added"), &e))?;
    gst::Element::link_many(refs.as_slice())
        .map_err(|e| pipeline_error(&format!("the {stream} branch would not link"), &e))?;
    let last = chain
        .last()
        .ok_or_else(|| SubError::new(codes::PIPELINE_FAILED, "an export branch has no elements"))?;
    last.link(muxer).map_err(|e| {
        pipeline_error(
            &format!("the {stream} encoder would not link to the muxer"),
            &e,
        )
    })?;
    Ok(())
}

/// The caps of the composited frames: what the compositor reads back.
fn video_caps(settings: &ExportSettings) -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .field("width", i32::try_from(settings.width).unwrap_or(i32::MAX))
        .field("height", i32::try_from(settings.height).unwrap_or(i32::MAX))
        .field(
            "framerate",
            gst::Fraction::new(
                i32::try_from(settings.frame_rate.numerator()).unwrap_or(i32::MAX),
                i32::try_from(settings.frame_rate.denominator()).unwrap_or(1),
            ),
        )
        .build()
}

/// The caps of the mix: interleaved `f32`, as the mixer produces it.
fn audio_caps(settings: &ExportSettings) -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field(
            "rate",
            i32::try_from(settings.sample_rate).unwrap_or(i32::MAX),
        )
        .field("channels", i32::from(settings.channels))
        .build()
}

/// The shared `appsrc` configuration: timestamps come from the buffers the
/// exporter stamps, the source is not live, and a full queue blocks the push
/// instead of growing without bound.
fn configure_appsrc(src: &AppSrc) {
    src.set_format(gst::Format::Time);
    src.set_is_live(false);
    src.set_property("block", true);
    src.set_max_bytes(APPSRC_MAX_BYTES);
    src.set_do_timestamp(false);
    src.set_property_from_str("stream-type", "stream");
}

/// Signals end of stream on one `appsrc`.
fn end_stream(src: &AppSrc) -> SubResult<()> {
    src.end_of_stream().map(|_| ()).map_err(|flow| {
        SubError::new(codes::PIPELINE_FAILED, "an export stream would not end")
            .with_detail("flow", format!("{flow:?}"))
    })
}

/// Builds one element, reporting a missing plugin as a pipeline failure.
fn make_element(name: &str) -> SubResult<gst::Element> {
    make_coded_element(name, codes::PIPELINE_FAILED)
}

/// Builds one element, reporting a missing plugin under `code`.
fn make_coded_element(name: &str, code: ErrorCode) -> SubResult<gst::Element> {
    gst::ElementFactory::make(name).build().map_err(|e| {
        SubError::wrap(code, format!("the export needs the {name} element"), &e)
            .with_detail("element", name.to_owned())
    })
}

/// Wraps a GStreamer failure as a pipeline error.
fn pipeline_error(message: &str, source: &dyn std::error::Error) -> SubError {
    SubError::wrap(codes::PIPELINE_FAILED, message.to_owned(), source)
}

#[cfg(test)]
mod tests {
    use super::{
        AUDIO_CODECS, AudioCodec, AudioFrameSource, CONTAINERS, Container, ExportSettings,
        PcmAudioSource, SolidFrames, VideoFrameSource, audio_nanos, video_parser,
    };
    use sub_time::Rational;

    fn settings(rate: Rational) -> ExportSettings {
        ExportSettings::new(16, 16, rate, Container::Mkv)
    }

    #[test]
    fn container_identifiers_round_trip_and_name_a_muxer() {
        for container in CONTAINERS {
            assert_eq!(Container::parse(container.as_str()), Some(container));
            assert!(!container.muxer().is_empty());
            assert_eq!(container.extension(), container.as_str());
        }
        assert_eq!(Container::parse("avi"), None);
    }

    #[test]
    fn audio_codec_identifiers_round_trip_and_name_encoders() {
        for codec in AUDIO_CODECS {
            assert_eq!(AudioCodec::parse(codec.as_str()), Some(codec));
            assert!(!codec.encoders().is_empty());
        }
        assert_eq!(AudioCodec::parse("mp3"), None);
    }

    #[test]
    fn every_video_codec_names_a_parser() {
        for codec in crate::CODECS {
            assert!(
                video_parser(codec).ends_with("parse"),
                "{codec} has no parser"
            );
        }
    }

    #[test]
    fn iso_containers_reject_matroska_only_audio() {
        assert!(Container::Mkv.accepts_audio(AudioCodec::Opus));
        assert!(!Container::Mp4.accepts_audio(AudioCodec::Opus));
        assert!(!Container::Mov.accepts_audio(AudioCodec::Flac));
        assert!(Container::Mp4.accepts_audio(AudioCodec::Aac));
    }

    #[test]
    fn validation_rejects_an_empty_canvas_and_a_mismatched_container() {
        let mut bad = settings(Rational::FPS_24);
        bad.width = 0;
        assert_eq!(
            bad.validate().unwrap_err().code.as_str(),
            "export.invalid_settings"
        );

        let mut mismatch = ExportSettings::new(16, 16, Rational::FPS_24, Container::Mov)
            .with_audio_codec(Some(AudioCodec::Opus));
        assert_eq!(
            mismatch.validate().unwrap_err().code.as_str(),
            "export.unsupported_combination"
        );
        mismatch = mismatch.with_audio_codec(Some(AudioCodec::Aac));
        mismatch.validate().expect("aac in mov is fine");

        let silent = settings(Rational::FPS_24)
            .with_audio_codec(None)
            .with_audio_format(0, 0);
        silent
            .validate()
            .expect("a video-only export needs no audio format");
    }

    #[test]
    fn frame_timestamps_are_exact_at_an_integer_rate() {
        let settings = settings(Rational::FPS_25);
        assert_eq!(settings.frame_start_nanos(0), 0);
        assert_eq!(settings.frame_start_nanos(1), 40_000_000);
        assert_eq!(settings.frame_start_nanos(25), 1_000_000_000);
        assert_eq!(settings.frame_start_nanos(25 * 3600), 3_600_000_000_000);
    }

    #[test]
    fn frame_timestamps_do_not_drift_at_ntsc_rates() {
        let settings = settings(Rational::FPS_23_976);
        // One hour of 24000/1001 is 3600 * 1001 / 1000 seconds exactly.
        let hour = 24_000 * 3600 / 1001;
        let expected = u128::from(hour) * 1001 * 1_000_000_000 / 24_000;
        assert_eq!(
            u128::from(settings.frame_start_nanos(hour)),
            expected,
            "the nanosecond stamp is the exact rational, not a rounded float"
        );
        // Every frame is within a nanosecond of its ideal spacing: no drift
        // accumulates, because each stamp is computed from the index.
        for index in [1_u64, 2, 1000, 100_000] {
            let stamp = u128::from(settings.frame_start_nanos(index));
            let ideal = u128::from(index) * 1001 * 1_000_000_000 / 24_000;
            assert_eq!(stamp, ideal);
        }
    }

    #[test]
    fn audio_spans_tile_the_video_frames_without_gaps_or_drift() {
        let settings = settings(Rational::FPS_29_97).with_audio_format(48_000, 2);
        let mut total = 0;
        for frame in 0..30_000_u64 {
            let span =
                settings.audio_frames_through(frame + 1) - settings.audio_frames_through(frame);
            total += span;
            assert_eq!(
                total,
                settings.audio_frames_through(frame + 1),
                "the per-frame spans always sum to the running total"
            );
        }
        // 30_000 frames of 30000/1001 fps is exactly 1001 seconds.
        assert_eq!(total, 1001 * 48_000);
    }

    #[test]
    fn audio_spans_are_whole_frames_at_an_integer_rate() {
        let settings = settings(Rational::FPS_24).with_audio_format(48_000, 2);
        for frame in 0..240_u64 {
            assert_eq!(
                settings.audio_frames_through(frame + 1) - settings.audio_frames_through(frame),
                2_000
            );
        }
    }

    #[test]
    fn audio_timestamps_match_the_frame_count() {
        assert_eq!(audio_nanos(0, 48_000), 0);
        assert_eq!(audio_nanos(48_000, 48_000), 1_000_000_000);
        assert_eq!(audio_nanos(24_000, 48_000), 500_000_000);
    }

    #[test]
    fn duration_is_the_frame_count_at_the_sequence_rate() {
        let settings = settings(Rational::FPS_23_976);
        let duration = settings.duration(48);
        assert_eq!(duration.value(), 48);
        assert_eq!(duration.rate(), Rational::FPS_23_976);
    }

    #[test]
    fn a_pcm_source_hands_out_whole_frames_then_runs_dry() {
        let mut source = PcmAudioSource::new(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let mut out = [0.0; 4];
        assert_eq!(source.read(&mut out, 2).expect("read"), 2);
        assert!(out.iter().eq([1.0, 2.0, 3.0, 4.0].iter()));
        assert_eq!(source.read(&mut out, 2).expect("read"), 1);
        assert!(out[..2].iter().eq([5.0, 6.0].iter()));
        assert_eq!(source.read(&mut out, 2).expect("read"), 0);
        assert_eq!(source.remaining(), 0);
    }

    #[test]
    fn solid_frames_run_out_after_the_count_it_was_given() {
        let mut frames = SolidFrames::new(16 * 16 * 4, 3);
        for _ in 0..3 {
            let frame = frames.next_frame().expect("a frame").expect("not done");
            assert_eq!(frame.len(), 16 * 16 * 4);
            assert_eq!(frame[3], 0xff, "frames are opaque");
        }
        assert!(frames.next_frame().expect("a frame").is_none());
    }
}
