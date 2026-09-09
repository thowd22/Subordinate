//! Per-clip video decoding: `uridecodebin` into an `appsink`, hardware
//! decoders preferred (docs/PLAN.md §5.2).
//!
//! [`Decoder`] is the reusable handle the preview, the thumbnailer and the
//! seek path all build on. It opens one file, plugs whatever decoder this
//! installation offers — hardware first — and hands out [`VideoFrame`]s in
//! presentation order, each carrying its presentation timestamp as an exact
//! [`RationalTime`] in nanoseconds. No timing value here is ever a float.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! let mut decoder = sub_media::Decoder::open(std::path::Path::new("/media/a.mp4"))?;
//! while let Some(frame) = decoder.next_frame()? {
//!     println!("{} ns", frame.pts().value());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Frames arrive as NV12 in system memory. A hardware decoder that produces
//! its own memory (VA surfaces, D3D12 textures) is downloaded and converted by
//! the `videoconvert` in front of the sink, so the frame layout a caller sees
//! never depends on which decoder was plugged; a caller that would rather have
//! planar I420 asks for it through [`DecoderOptions::format`]. Zero-copy import
//! is a post-MVP optimisation.
//!
//! A video file's audio comes off this same pipeline (decision-4): asking for
//! [`StreamSelection::VideoAndAudio`] or [`StreamSelection::AudioOnly`] adds an
//! `audioconvert` and a second `appsink`, and [`Decoder::next_audio_block`]
//! hands out interleaved `f32` PCM with sample-accurate positions. See the
//! [`audio`](crate::audio) module for the shape of those blocks and for the
//! gains a mono or 5.1 source is folded to stereo with.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use sub_media::{DecoderOptions, StreamSelection};
//!
//! let options = DecoderOptions {
//!     streams: StreamSelection::AudioOnly,
//!     ..DecoderOptions::default()
//! };
//! let mut decoder = sub_media::Decoder::open_with(std::path::Path::new("/media/a.mp4"), options)?;
//! let rate = decoder.audio_format().expect("audio only").sample_rate;
//! while let Some(block) = decoder.next_audio_block()? {
//!     println!("{} frames at {rate} Hz", block.frames());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Dropping a [`Decoder`] sets its pipeline to `Null`, which stops the
//! streaming threads and releases the decoder and the file handle.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use gstreamer_video::{VideoFormat, VideoFrameExt, VideoInfo};
use sub_core::{ResultExt, SubError, SubResult};
use sub_time::{RationalTime, Rounding};

use crate::audio::{
    AudioBlock, AudioChannels, AudioFormat, MAX_CHANNELS, StereoDownmix, frames_at,
    layout_from_roles, roles_from_positions,
};
use crate::codes;
use crate::probe::NANOSECONDS;

/// Pixel format every [`VideoFrame`] is delivered in.
///
/// NV12 is what the MVP compositor uploads and what every hardware decoder in
/// the preferred set produces natively, so the conversion in front of the sink
/// is usually a no-op. The documented fallback for a source the converter
/// cannot express as NV12 (an alpha or high-bit-depth stream) is I420, which is
/// requested second in the sink's caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameFormat {
    /// 8-bit 4:2:0, one luma plane and one interleaved chroma plane.
    Nv12,
    /// 8-bit 4:2:0 planar, the documented fallback.
    I420,
}

impl FrameFormat {
    /// The GStreamer format name, as it appears in caps.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nv12 => "NV12",
            Self::I420 => "I420",
        }
    }

    /// Number of planes the format carries.
    pub fn plane_count(self) -> usize {
        match self {
            Self::Nv12 => 2,
            Self::I420 => 3,
        }
    }

    /// Maps a GStreamer video format onto the two this decoder delivers.
    fn from_video_format(format: VideoFormat) -> Option<Self> {
        match format {
            VideoFormat::Nv12 => Some(Self::Nv12),
            VideoFormat::I420 => Some(Self::I420),
            _ => None,
        }
    }
}

/// Which streams of a file a [`Decoder`] pulls out of it.
///
/// A video file is demuxed once (decision-4): the audio a caller needs comes
/// off the same `uridecodebin` as the pictures rather than out of a second
/// pipeline, so nothing has to be parsed or seeked twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StreamSelection {
    /// Decode the video stream only. Any audio is dropped by decodebin.
    #[default]
    Video,
    /// Decode both, from one demux. The caller must pull from both branches:
    /// each `appsink` holds a bounded queue, so a caller that only ever pulls
    /// frames eventually stalls the audio branch and with it the pipeline.
    VideoAndAudio,
    /// Decode the audio stream only; video pads are discarded. This is what
    /// the waveform job and an audio-led scrub use.
    AudioOnly,
}

impl StreamSelection {
    /// Whether a video branch is built.
    fn wants_video(self) -> bool {
        matches!(self, Self::Video | Self::VideoAndAudio)
    }

    /// Whether an audio branch is built.
    fn wants_audio(self) -> bool {
        matches!(self, Self::VideoAndAudio | Self::AudioOnly)
    }
}

/// Whether a decoder handle should try to use a hardware decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HardwarePreference {
    /// Rank the known hardware decoders above the software ones, so autoplug
    /// picks a hardware decoder whenever this machine has one. Software is
    /// still used when it does not.
    #[default]
    Prefer,
    /// Leave the registry ranks alone and take whatever autoplug picks. Used by
    /// tests that must decode identically everywhere, and as the escape hatch
    /// for a broken driver.
    Software,
}

/// How a [`Decoder`] should be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecoderOptions {
    /// Which streams are decoded.
    pub streams: StreamSelection,
    /// How many channels each [`AudioBlock`] carries. Ignored when no audio is
    /// decoded.
    pub audio_channels: AudioChannels,
    /// Whether hardware decoders are preferred.
    pub hardware: HardwarePreference,
    /// Pixel format frames are delivered in. NV12 is what the compositor
    /// uploads; I420 is the documented fallback for a caller that would rather
    /// take a software decoder's own planar output than have it converted.
    pub format: FrameFormat,
    /// How long [`Decoder::next_frame`] or [`Decoder::next_audio_block`] waits
    /// before giving up with `media.decode_timeout`. It bounds a stalled
    /// pipeline, not the whole decode: the budget applies to each frame, to
    /// each audio block, and to the wait for the audio format at open.
    pub frame_timeout: Duration,
    /// How far ahead of the current position [`Decoder::seek_to`] decodes
    /// forward instead of issuing a new keyframe seek.
    ///
    /// A target just ahead of the playhead is almost always inside the GOP the
    /// decoder is already in, where a keyframe seek would land back on the
    /// keyframe the decoder has already passed and decode the same pictures
    /// again. The window is that GOP budget, expressed in time because the GOP
    /// length of a file is not known until it has been decoded: the default of
    /// two seconds covers the one-second GOPs cameras and the fixtures use, and
    /// a caller that knows its sources can widen or narrow it.
    pub forward_decode_window: Duration,
}

impl Default for DecoderOptions {
    fn default() -> Self {
        Self {
            streams: StreamSelection::Video,
            audio_channels: AudioChannels::StereoDownmix,
            hardware: HardwarePreference::Prefer,
            format: FrameFormat::Nv12,
            frame_timeout: Duration::from_secs(10),
            forward_decode_window: Duration::from_secs(2),
        }
    }
}

/// One decoded picture, with its presentation timestamp.
///
/// The frame owns its mapped buffer, so plane data stays valid for as long as
/// the frame does and no copy is made on the way out of GStreamer.
pub struct VideoFrame {
    frame: gstreamer_video::VideoFrame<gstreamer_video::video_frame::Readable>,
    format: FrameFormat,
    pts: RationalTime,
    duration: Option<RationalTime>,
}

impl VideoFrame {
    /// Presentation timestamp, exact, counted in nanoseconds from the start of
    /// the stream. Rescale to a sequence rate to get a frame number.
    pub fn pts(&self) -> RationalTime {
        self.pts
    }

    /// Frame duration when the decoder reported one; a variable-frame-rate
    /// source often does not.
    pub fn duration(&self) -> Option<RationalTime> {
        self.duration
    }

    /// Pixel format of the plane data.
    pub fn format(&self) -> FrameFormat {
        self.format
    }

    /// Picture width in pixels.
    pub fn width(&self) -> u32 {
        self.frame.width()
    }

    /// Picture height in pixels.
    pub fn height(&self) -> u32 {
        self.frame.height()
    }

    /// Resident size of the picture in bytes, every plane and its padding
    /// included.
    ///
    /// This is what the frame costs the process, so it is what
    /// [`FrameCache`](crate::FrameCache) charges against its budget.
    pub fn byte_size(&self) -> usize {
        self.frame.size()
    }

    /// Bytes of one plane, or `None` when the index is past the last plane.
    pub fn plane_data(&self, plane: u32) -> Option<&[u8]> {
        self.frame.plane_data(plane).ok()
    }

    /// Row stride of one plane in bytes, or `None` when the index is past the
    /// last plane. A stride is never assumed to equal the width.
    pub fn plane_stride(&self, plane: u32) -> Option<u32> {
        let strides = self.frame.plane_stride();
        let stride = *strides.get(plane as usize)?;
        u32::try_from(stride).ok()
    }
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("pts_ns", &self.pts.value())
            .field("duration", &self.duration.map(RationalTime::value))
            .field("format", &self.format)
            .field("width", &self.width())
            .field("height", &self.height())
            .finish_non_exhaustive()
    }
}

/// A decoding pipeline for one media file.
///
/// See the [module documentation](self) for the pipeline shape and the
/// hardware preference.
pub struct Decoder {
    pipeline: gst::Pipeline,
    sink: Option<AppSink>,
    audio: Option<AudioBranch>,
    chosen: Arc<Mutex<Option<String>>>,
    frame_timeout: Duration,
    forward_window: RationalTime,
    saw_video: Arc<AtomicBool>,
    delivered: u64,
    finished: bool,
    position: Option<RationalTime>,
    seeks: u64,
    frames_since_seek: u64,
    prerolled: bool,
}

/// The audio half of a decode: the second `appsink`, the shape it negotiated,
/// and the buffers the samples are converted through.
struct AudioBranch {
    sink: AppSink,
    format: AudioFormat,
    /// The fold to stereo, or `None` when the source channels pass through.
    downmix: Option<StereoDownmix>,
    /// Source-order samples of the buffer being converted.
    source: Vec<f32>,
    /// Samples handed out, folded when a fold is in force.
    delivered: Vec<f32>,
    /// Frame the next block starts on, once the first buffer has set it.
    next_frame: Option<i64>,
    /// Blocks delivered so far.
    blocks: u64,
    /// True once the audio branch reached end of stream.
    finished: bool,
}

/// Checks `path` is a readable file and turns it into a `file://` URI.
///
/// # Errors
///
/// Returns `media.file_unreadable` when the path cannot be read, is not a
/// file, or cannot be expressed as a URI.
pub(crate) fn readable_file_uri(path: &Path) -> SubResult<gst::glib::GString> {
    let metadata = std::fs::metadata(path)
        .sub_context_with(codes::FILE_UNREADABLE, || "media file cannot be read")
        .map_err(|e| e.with_detail("path", path.display().to_string()))?;
    if !metadata.is_file() {
        return Err(
            SubError::new(codes::FILE_UNREADABLE, "media path is not a file")
                .with_detail("path", path.display().to_string()),
        );
    }
    gst::glib::filename_to_uri(path, None)
        .map_err(|e| SubError::wrap(codes::FILE_UNREADABLE, "media path is not a URI", &e))
        .map_err(|e| e.with_detail("path", path.display().to_string()))
}

impl Decoder {
    /// Opens `path` for decoding with the default options.
    ///
    /// # Errors
    ///
    /// Returns `media.file_unreadable` when the path is not a readable file,
    /// `media.unsupported` when a required GStreamer element is missing and
    /// `media.decode_failed` when the pipeline cannot be built or started.
    pub fn open(path: &Path) -> SubResult<Self> {
        Self::open_with(path, DecoderOptions::default())
    }

    /// Opens `path` for decoding.
    ///
    /// # Errors
    ///
    /// As [`Decoder::open`].
    pub fn open_with(path: &Path, options: DecoderOptions) -> SubResult<Self> {
        gst::init()
            .map_err(|e| SubError::wrap(codes::INIT_FAILED, "GStreamer failed to start", &e))?;

        let uri = readable_file_uri(path)?;

        if options.hardware == HardwarePreference::Prefer {
            prefer_hardware_decoders();
        }

        let pipeline = gst::Pipeline::new();
        let source = gst::ElementFactory::make("uridecodebin")
            .property("uri", &uri)
            // Only the streams named in `caps` are exposed; decodebin disposes
            // of the others itself, so no pad is left unlinked to stall the
            // flow.
            .property("expose-all-streams", false)
            .property("caps", raw_caps(options.streams))
            .build()
            .sub_context(codes::UNSUPPORTED, "uridecodebin is unavailable")?;
        pipeline
            .add(&source)
            .sub_context(codes::DECODE_FAILED, "could not build the decode pipeline")?;

        let video_sink = if options.streams.wants_video() {
            Some(build_video_branch(&pipeline, options.format)?)
        } else {
            None
        };
        let audio_sink = if options.streams.wants_audio() {
            Some(build_audio_branch(&pipeline)?)
        } else {
            None
        };

        // A file with several streams of one type exposes several pads; the
        // first of each type is decoded and any later one is discarded rather
        // than left dangling.
        let saw_video = Arc::new(AtomicBool::new(false));
        let saw_audio = Arc::new(AtomicBool::new(false));
        link_decoded_pads(
            &source,
            &pipeline,
            &LinkTargets {
                video: video_sink.as_ref().map(|sink| (sink, &saw_video)),
                audio: audio_sink.as_ref().map(|sink| (sink, &saw_audio)),
            },
        )?;

        let no_more_pads = Arc::new(AtomicBool::new(false));
        let pads_done = Arc::clone(&no_more_pads);
        source.connect_no_more_pads(move |_| pads_done.store(true, Ordering::SeqCst));

        let chosen = Arc::new(Mutex::new(None));
        watch_chosen_decoder(&pipeline, &chosen);

        let video_sink = video_sink
            .map(|sink| {
                sink.downcast::<AppSink>()
                    .map_err(|_| SubError::new(codes::DECODE_FAILED, "appsink has the wrong type"))
            })
            .transpose()?;
        let audio_sink = audio_sink
            .map(|sink| {
                sink.downcast::<AppSink>()
                    .map_err(|_| SubError::new(codes::DECODE_FAILED, "appsink has the wrong type"))
            })
            .transpose()?;

        pipeline.set_state(gst::State::Playing).map_err(|e| {
            SubError::wrap(
                codes::DECODE_FAILED,
                "the decode pipeline would not start",
                &e,
            )
            .with_detail("path", path.display().to_string())
        })?;

        // The audio shape is negotiated, not declared, so the decoder waits for
        // it here: a caller then always knows the sample rate and the layout,
        // and a file opened for audio that carries none fails at open rather
        // than at the first pull.
        let audio = match audio_sink {
            Some(sink) => open_audio_branch(
                &pipeline,
                sink,
                &AudioNegotiation {
                    saw_audio: &saw_audio,
                    no_more_pads: &no_more_pads,
                },
                options,
            )
            .map_err(|e| {
                let _ = pipeline.set_state(gst::State::Null);
                e.with_detail("path", path.display().to_string())
            })?,
            None => None,
        };
        Ok(Self {
            pipeline,
            sink: video_sink,
            audio,
            chosen,
            frame_timeout: options.frame_timeout,
            forward_window: duration_time(options.forward_decode_window),
            saw_video,
            delivered: 0,
            finished: false,
            position: None,
            seeks: 0,
            frames_since_seek: 0,
            prerolled: false,
        })
    }

    /// Pulls the next frame, or `None` once the stream has ended.
    ///
    /// Frames come out in presentation order, so their timestamps are
    /// non-decreasing.
    ///
    /// # Errors
    ///
    /// Returns `media.decode_failed` when the pipeline reports an error or
    /// delivers a frame in a shape this decoder does not understand,
    /// `media.decode_timeout` when no frame arrives within the frame budget,
    /// `media.no_video_stream` when the file ends without a single frame, and
    /// the same code when this decoder was opened for audio only.
    pub fn next_frame(&mut self) -> SubResult<Option<VideoFrame>> {
        if self.finished {
            return Ok(None);
        }
        let sink = self.sink.clone().ok_or_else(|| {
            SubError::new(
                codes::NO_VIDEO_STREAM,
                "this decoder was opened for audio only",
            )
        })?;
        let deadline = Instant::now() + self.frame_timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            // Waiting for the whole budget in one call would hide an error the
            // pipeline has already posted, so the wait is sliced and the bus is
            // read between slices.
            if let Some(sample) = sink.try_pull_sample(clock_time(left.min(POLL_SLICE))) {
                let frame = Self::frame_from_sample(&sample)?;
                self.delivered += 1;
                self.prerolled = true;
                self.frames_since_seek += 1;
                self.position = Some(frame.pts());
                return Ok(Some(frame));
            }
            if let Some(err) = self.pipeline_error() {
                self.finished = true;
                // A file that never exposed a video pad usually fails as a
                // pipeline error; "no video stream" says far more than that.
                return Err(self.no_video_stream().unwrap_or(err));
            }
            if sink.is_eos() {
                self.finished = true;
                return self.no_video_stream().map_or(Ok(None), Err);
            }
            if left.is_zero() {
                self.finished = true;
                return Err(SubError::new(
                    codes::DECODE_TIMEOUT,
                    "no decoded frame arrived within the frame budget",
                )
                .with_detail("timeout_ms", self.frame_timeout.as_millis().to_string()));
            }
        }
    }

    /// Seeks to `target` and returns the first frame at or after it, or `None`
    /// when the stream ends before the target.
    ///
    /// This is the frame-accurate seek the scrub bar and split-at-playhead are
    /// built on (docs/PLAN.md §5.2). A container seek lands on a keyframe, so
    /// on a long-GOP source the frame it produces can be seconds away from the
    /// one that was asked for. The two-step answer is the one every editor
    /// uses: seek backwards to the keyframe at or before the target with a
    /// flushing `KEY_UNIT` seek, then decode forward, discarding frames until
    /// one carries a presentation timestamp at or after `target`. Every
    /// comparison here is exact [`RationalTime`] arithmetic, so a target
    /// expressed at a frame rate matches a nanosecond timestamp with no
    /// tolerance and no float.
    ///
    /// A target that is ahead of the current position but within
    /// [`DecoderOptions::forward_decode_window`] is reached by decoding forward
    /// alone: no seek is issued, because the target is in the GOP the decoder
    /// is already inside and re-seeking would decode the same pictures twice.
    /// [`Decoder::seek_count`] reports how many seeks were actually issued.
    ///
    /// A negative target is treated as the start of the stream.
    ///
    /// # Errors
    ///
    /// Returns `media.seek_failed` when the pipeline refuses the seek or never
    /// reaches a seekable state, plus every error [`Decoder::next_frame`]
    /// returns while the frames up to the target are decoded.
    pub fn seek_to(&mut self, target: RationalTime) -> SubResult<Option<VideoFrame>> {
        let target = if target.is_negative() {
            RationalTime::zero(target.rate())
        } else {
            target
        };
        if plan_seek(self.position, target, self.forward_window) == SeekPlan::Reseek {
            self.keyframe_seek(target)?;
        }
        let mut backoff = duration_time(SEEK_BACKOFF);
        let mut aim = target;
        loop {
            let Some(frame) = self.next_frame()? else {
                return Ok(None);
            };
            if frame.pts() < target {
                // Between the keyframe and the target: decoded only to build
                // the reference chain the target frame needs.
                continue;
            }
            // The first frame after a seek can be *after* the target even
            // though a keyframe sits before it: a container whose timestamps
            // do not start at zero seeks in a stream time that is offset from
            // the presentation timestamps, so the demuxer picks the keyframe
            // before an instant that is not the one that was asked for. The
            // answer is to aim further back and decode forward from there.
            if self.frames_since_seek == 1
                && frame.pts() > target
                && let Some(earlier) = earlier_target(aim, backoff)
            {
                self.keyframe_seek(earlier)?;
                aim = earlier;
                backoff = backoff + backoff;
                continue;
            }
            return Ok(Some(frame));
        }
    }

    /// Presentation timestamp of the last frame this decoder delivered, or
    /// `None` before the first one and immediately after a seek.
    pub fn position(&self) -> Option<RationalTime> {
        self.position
    }

    /// How many flushing seeks this decoder has issued.
    ///
    /// Forward seeks inside the GOP window do not add to it, which is what
    /// makes scrubbing forward cheap.
    pub fn seek_count(&self) -> u64 {
        self.seeks
    }

    /// Issues the flushing keyframe-backward seek.
    fn keyframe_seek(&mut self, target: RationalTime) -> SubResult<()> {
        self.wait_until_seekable()?;
        // Flooring keeps the seek at or before the requested instant: a target
        // rounded up could skip past the very frame that was asked for.
        let nanos = target
            .checked_rescaled_to_rounding(NANOSECONDS, Rounding::Floor)
            .and_then(|time| u64::try_from(time.value()).ok())
            .ok_or_else(|| {
                SubError::new(codes::SEEK_FAILED, "seek target is not a reachable instant")
            })?;
        self.pipeline
            .seek_simple(
                // FLUSH drops what is in flight so the frames that arrive next
                // are the ones after the seek; KEY_UNIT with SNAP_BEFORE lands
                // on the keyframe at or before the target, which is what makes
                // decoding forward from there possible at all.
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT | gst::SeekFlags::SNAP_BEFORE,
                gst::ClockTime::from_nseconds(nanos),
            )
            .map_err(|e| {
                SubError::wrap(codes::SEEK_FAILED, "the pipeline refused the seek", &e)
                    .with_detail("target_ns", nanos.to_string())
            })?;
        self.seeks += 1;
        self.frames_since_seek = 0;
        // A flushing seek also clears an end-of-stream, so a decoder that ran
        // to the end is usable again.
        self.finished = false;
        self.position = None;
        Ok(())
    }

    /// Waits for the pipeline to finish starting, once.
    ///
    /// A seek sent before the pipeline has pre-rolled is dropped by the
    /// demuxer, so the first seek blocks until the state change completes.
    fn wait_until_seekable(&mut self) -> SubResult<()> {
        if self.prerolled {
            return Ok(());
        }
        let (result, _, _) = self.pipeline.state(clock_time(self.frame_timeout));
        match result {
            Ok(gst::StateChangeSuccess::Success | gst::StateChangeSuccess::NoPreroll) => {
                self.prerolled = true;
                Ok(())
            }
            Ok(gst::StateChangeSuccess::Async) | Err(_) => {
                Err(self.pipeline_error().unwrap_or_else(|| {
                    SubError::new(codes::SEEK_FAILED, "the pipeline never became seekable")
                }))
            }
        }
    }

    /// The `media.no_video_stream` error, when this decode has produced no
    /// frame and never will: either no video pad was exposed at all, or the one
    /// that was decoded nothing.
    fn no_video_stream(&self) -> Option<SubError> {
        if self.delivered > 0 {
            return None;
        }
        let message = if self.saw_video.load(Ordering::SeqCst) {
            "the file decoded no video frames"
        } else {
            "the file carries no video stream to decode"
        };
        Some(SubError::new(codes::NO_VIDEO_STREAM, message))
    }

    /// The decoder element decodebin plugged, once one has been plugged.
    ///
    /// Available after the first frame; before that the pipeline may still be
    /// autoplugging. The name is a GStreamer element name such as
    /// `vah264dec` or `avdec_h264`.
    pub fn decoder_element(&self) -> Option<String> {
        self.chosen.lock().ok().and_then(|name| name.clone())
    }

    /// The shape of the audio this decoder delivers: sample rate, source
    /// channel count and layout, and the channel count each block carries.
    ///
    /// Known as soon as the decoder is open — the negotiation is waited for
    /// there — and `None` when no audio was asked for, or when a file opened
    /// with [`StreamSelection::VideoAndAudio`] carries none.
    pub fn audio_format(&self) -> Option<&AudioFormat> {
        self.audio.as_ref().map(|branch| &branch.format)
    }

    /// Pulls the next run of audio frames, or `None` once the audio stream has
    /// ended.
    ///
    /// Blocks are interleaved `f32`, stereo unless [`AudioChannels::Source`]
    /// was asked for, and carry a sample-accurate start position at the
    /// source's own sample rate. Their length is whatever the decoder produced;
    /// a caller that needs fixed-size buffers accumulates them.
    ///
    /// The samples borrow the decoder's own buffer, which the next call
    /// overwrites.
    ///
    /// # Errors
    ///
    /// Returns `media.no_audio_stream` when this decoder has no audio branch,
    /// `media.decode_failed` when the pipeline reports an error or the stream
    /// changes shape mid-file, and `media.decode_timeout` when no block arrives
    /// within the frame budget.
    pub fn next_audio_block(&mut self) -> SubResult<Option<AudioBlock<'_>>> {
        let Self {
            pipeline,
            audio,
            frame_timeout,
            ..
        } = self;
        let branch = audio.as_mut().ok_or_else(|| {
            SubError::new(
                codes::NO_AUDIO_STREAM,
                "this decoder has no audio stream to pull from",
            )
        })?;
        if branch.finished {
            return Ok(None);
        }
        let deadline = Instant::now() + *frame_timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if let Some(sample) = branch
                .sink
                .try_pull_sample(clock_time(left.min(POLL_SLICE)))
            {
                let (start, frames) = branch.fill_from_sample(&sample)?;
                if frames == 0 {
                    // An empty buffer carries no frames; it is skipped rather
                    // than handed out as a zero-length block.
                    continue;
                }
                branch.blocks += 1;
                return Ok(Some(AudioBlock {
                    start: RationalTime::new(start, branch.format.frame_rate()),
                    sample_rate: branch.format.sample_rate,
                    channels: branch.format.channels,
                    samples: &branch.delivered,
                }));
            }
            if let Some(err) = pipeline_error(pipeline) {
                branch.finished = true;
                return Err(err);
            }
            if branch.sink.is_eos() {
                branch.finished = true;
                return Ok(None);
            }
            if left.is_zero() {
                branch.finished = true;
                return Err(SubError::new(
                    codes::DECODE_TIMEOUT,
                    "no decoded audio arrived within the frame budget",
                )
                .with_detail("timeout_ms", frame_timeout.as_millis().to_string()));
            }
        }
    }

    /// Turns one appsink sample into a frame, mapping its buffer for reading.
    fn frame_from_sample(sample: &gst::Sample) -> SubResult<VideoFrame> {
        let caps = sample
            .caps()
            .ok_or_else(|| SubError::new(codes::DECODE_FAILED, "decoded sample has no caps"))?;
        let info = VideoInfo::from_caps(caps)
            .map_err(|e| SubError::wrap(codes::DECODE_FAILED, "decoded caps are not video", &e))?;
        let format = FrameFormat::from_video_format(info.format()).ok_or_else(|| {
            SubError::new(codes::DECODE_FAILED, "unexpected decoded pixel format")
                .with_detail("format", info.format().to_string())
        })?;
        let buffer = sample
            .buffer_owned()
            .ok_or_else(|| SubError::new(codes::DECODE_FAILED, "decoded sample has no buffer"))?;
        let pts = buffer.pts().ok_or_else(|| {
            SubError::new(codes::DECODE_FAILED, "decoded frame carries no timestamp")
        })?;
        let duration = buffer.duration().map(|d| nanoseconds(d.nseconds()));
        let frame = gstreamer_video::VideoFrame::from_buffer_readable(buffer, &info)
            .map_err(|_| SubError::new(codes::DECODE_FAILED, "decoded frame could not be read"))?;
        Ok(VideoFrame {
            frame,
            format,
            pts: nanoseconds(pts.nseconds()),
            duration,
        })
    }

    /// The first error message sitting on the bus, if any.
    fn pipeline_error(&self) -> Option<SubError> {
        pipeline_error(&self.pipeline)
    }
}

/// The first error message sitting on a pipeline's bus, if any.
fn pipeline_error(pipeline: &gst::Pipeline) -> Option<SubError> {
    let bus = pipeline.bus()?;
    while let Some(message) = bus.pop() {
        if let gst::MessageView::Error(err) = message.view() {
            return Some(SubError::wrap(
                codes::DECODE_FAILED,
                "the decode pipeline failed",
                &err.error(),
            ));
        }
    }
    None
}

impl AudioBranch {
    /// Converts one appsink sample into the delivery buffer, returning the
    /// frame the block starts on and how many frames it holds.
    ///
    /// Positions are counted in frames from the first buffer's timestamp, so
    /// consecutive blocks are contiguous and sample-accurate whatever rounding
    /// the container applied to its own nanosecond timestamps.
    fn fill_from_sample(&mut self, sample: &gst::Sample) -> SubResult<(i64, usize)> {
        let caps = sample
            .caps()
            .ok_or_else(|| SubError::new(codes::DECODE_FAILED, "decoded audio has no caps"))?;
        let info = gstreamer_audio::AudioInfo::from_caps(caps)
            .map_err(|e| SubError::wrap(codes::DECODE_FAILED, "decoded caps are not audio", &e))?;
        let channels = u16::try_from(info.channels()).unwrap_or(u16::MAX);
        if info.rate() != self.format.sample_rate || channels != self.format.source_channels {
            return Err(SubError::new(
                codes::DECODE_FAILED,
                "the audio stream changed shape mid-file",
            )
            .with_detail("sample_rate", info.rate().to_string())
            .with_detail("channels", channels.to_string()));
        }
        let buffer = sample
            .buffer()
            .ok_or_else(|| SubError::new(codes::DECODE_FAILED, "decoded audio has no buffer"))?;
        let map = buffer
            .map_readable()
            .map_err(|_| SubError::new(codes::DECODE_FAILED, "decoded audio could not be read"))?;

        // The sink negotiated F32LE, so the bytes are four-byte little-endian
        // samples whatever this machine's own endianness is.
        self.source.clear();
        self.source.extend(
            map.as_slice()
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        );
        if let Some(downmix) = &self.downmix {
            downmix.apply(&self.source, &mut self.delivered);
        } else {
            self.delivered.clear();
            self.delivered.extend_from_slice(&self.source);
        }

        let frames = self.source.len() / usize::from(self.format.source_channels);
        let start = match self.next_frame {
            Some(next) => next,
            None => buffer
                .pts()
                .map_or(0, |pts| frames_at(pts.nseconds(), self.format.frame_rate())),
        };
        self.next_frame = Some(start.saturating_add(i64::try_from(frames).unwrap_or(i64::MAX)));
        Ok((start, frames))
    }
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder")
            .field("decoder_element", &self.decoder_element())
            .field("frames_delivered", &self.delivered)
            .field("position_ns", &self.position.map(RationalTime::value))
            .field("seeks", &self.seeks)
            .field("audio_format", &self.audio_format())
            .field(
                "audio_blocks_delivered",
                &self.audio.as_ref().map(|branch| branch.blocks),
            )
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // Null stops the streaming threads and releases the decoder, the
        // hardware context and the file handle. A pipeline that refuses is
        // logged rather than panicked on: dropping must not unwind.
        if let Err(err) = self.pipeline.set_state(gst::State::Null) {
            tracing::warn!(%err, "decode pipeline did not shut down cleanly");
        }
    }
}

/// The flags the wait for the audio negotiation watches.
struct AudioNegotiation<'a> {
    /// Set once a decoded audio pad has been linked into the branch.
    saw_audio: &'a Arc<AtomicBool>,
    /// Set once decodebin has exposed every pad it is going to.
    no_more_pads: &'a Arc<AtomicBool>,
}

/// Waits for the audio branch to negotiate and builds it.
///
/// Returns `Ok(None)` when the file carries no audio and the caller asked for
/// video as well; a decode opened for audio only fails instead.
fn open_audio_branch(
    pipeline: &gst::Pipeline,
    sink: AppSink,
    negotiation: &AudioNegotiation<'_>,
    options: DecoderOptions,
) -> SubResult<Option<AudioBranch>> {
    let negotiated = wait_for_audio_format(
        pipeline,
        &sink,
        negotiation.saw_audio,
        negotiation.no_more_pads,
        options.frame_timeout,
        options.audio_channels,
    )?;
    match negotiated {
        Some((format, downmix)) => Ok(Some(AudioBranch {
            sink,
            format,
            downmix,
            source: Vec::new(),
            delivered: Vec::new(),
            next_frame: None,
            blocks: 0,
            finished: false,
        })),
        None if options.streams == StreamSelection::AudioOnly => Err(SubError::new(
            codes::NO_AUDIO_STREAM,
            "the file carries no audio stream to decode",
        )),
        None => Ok(None),
    }
}

/// Caps decodebin treats as decoded, for the streams this decode wants: raw
/// video including the memory features a hardware decoder attaches, raw audio,
/// or both.
fn raw_caps(streams: StreamSelection) -> gst::Caps {
    let mut caps = gst::Caps::new_empty();
    if let Some(caps) = caps.get_mut() {
        if streams.wants_video() {
            caps.append_structure_full(
                gst::Structure::new_empty("video/x-raw"),
                Some(gst::CapsFeatures::new_any()),
            );
        }
        if streams.wants_audio() {
            caps.append_structure(gst::Structure::new_empty("audio/x-raw"));
        }
    }
    caps
}

/// Builds the video half of the pipeline — `videoconvert` into an `appsink` —
/// and returns the sink, still to be linked to a decoded pad.
fn build_video_branch(pipeline: &gst::Pipeline, format: FrameFormat) -> SubResult<gst::Element> {
    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .sub_context(codes::UNSUPPORTED, "videoconvert is unavailable")?;
    let sink = gst::ElementFactory::make("appsink")
        .property("sync", false)
        .property("max-buffers", 2_u32)
        .property("caps", sink_caps(format))
        .build()
        .sub_context(codes::UNSUPPORTED, "appsink is unavailable")?;
    pipeline
        .add_many([&convert, &sink])
        .sub_context(codes::DECODE_FAILED, "could not build the decode pipeline")?;
    convert
        .link(&sink)
        .sub_context(codes::DECODE_FAILED, "could not link the decode pipeline")?;
    Ok(sink)
}

/// Builds the audio half of the pipeline — `audioconvert` into a second
/// `appsink` — and returns the sink, still to be linked to a decoded pad.
///
/// The sink names the sample format only, so the source's own rate and channel
/// count negotiate through unchanged: resampling and the fold to stereo are
/// this crate's business, not the converter's.
fn build_audio_branch(pipeline: &gst::Pipeline) -> SubResult<gst::Element> {
    let convert = gst::ElementFactory::make("audioconvert")
        .build()
        .sub_context(codes::UNSUPPORTED, "audioconvert is unavailable")?;
    let sink = gst::ElementFactory::make("appsink")
        .property("sync", false)
        .property("max-buffers", AUDIO_QUEUE_BUFFERS)
        .property("caps", audio_sink_caps())
        .build()
        .sub_context(codes::UNSUPPORTED, "appsink is unavailable")?;
    pipeline
        .add_many([&convert, &sink])
        .sub_context(codes::DECODE_FAILED, "could not build the decode pipeline")?;
    convert
        .link(&sink)
        .sub_context(codes::DECODE_FAILED, "could not link the decode pipeline")?;
    Ok(sink)
}

/// The branches a decoded pad can be linked into, each with the flag that
/// records whether that branch has already claimed a stream.
struct LinkTargets<'a> {
    video: Option<(&'a gst::Element, &'a Arc<AtomicBool>)>,
    audio: Option<(&'a gst::Element, &'a Arc<AtomicBool>)>,
}

/// Links each decoded pad into the branch that wants it, discarding the pads
/// of a second stream of a type this decode has already claimed.
fn link_decoded_pads(
    source: &gst::Element,
    pipeline: &gst::Pipeline,
    targets: &LinkTargets<'_>,
) -> SubResult<()> {
    /// Resolves a branch's sink element to the pad a decoded pad links into.
    fn entry_pad(sink: &gst::Element) -> SubResult<gst::Pad> {
        sink.static_pad("sink")
            .and_then(|pad| pad.peer())
            .and_then(|peer| peer.parent_element())
            .and_then(|convert| convert.static_pad("sink"))
            .ok_or_else(|| SubError::new(codes::DECODE_FAILED, "a decode branch has no sink pad"))
    }

    let video = targets
        .video
        .map(|(sink, taken)| Ok::<_, SubError>((entry_pad(sink)?, Arc::clone(taken))))
        .transpose()?;
    let audio = targets
        .audio
        .map(|(sink, taken)| Ok::<_, SubError>((entry_pad(sink)?, Arc::clone(taken))))
        .transpose()?;

    let weak_pipeline = pipeline.downgrade();
    source.connect_pad_added(move |_, pad| {
        let media = pad_media_type(pad);
        let target = match media.as_deref() {
            Some(name) if name.starts_with("video/") => video.as_ref(),
            Some(name) if name.starts_with("audio/") => audio.as_ref(),
            _ => None,
        };
        let Some((entry, taken)) = target else {
            if let Some(pipeline) = weak_pipeline.upgrade() {
                discard_pad(&pipeline, pad);
            }
            return;
        };
        if taken.swap(true, Ordering::SeqCst) {
            if let Some(pipeline) = weak_pipeline.upgrade() {
                discard_pad(&pipeline, pad);
            }
            return;
        }
        if let Err(err) = pad.link(entry) {
            tracing::warn!(%err, media = ?media, "decoded pad could not be linked");
        }
    });
    Ok(())
}

/// The media type of a decoded pad, for example `video/x-raw`.
fn pad_media_type(pad: &gst::Pad) -> Option<String> {
    let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
    Some(caps.structure(0)?.name().to_string())
}

/// Waits for the audio branch to negotiate, and reports the shape it settled
/// on together with the fold that shape needs.
///
/// Returns `Ok(None)` when the file exposed no audio pad at all: whether that
/// is an error depends on what the caller asked for.
fn wait_for_audio_format(
    pipeline: &gst::Pipeline,
    sink: &AppSink,
    saw_audio: &Arc<AtomicBool>,
    no_more_pads: &Arc<AtomicBool>,
    timeout: Duration,
    channels: AudioChannels,
) -> SubResult<Option<(AudioFormat, Option<StereoDownmix>)>> {
    let pad = sink
        .static_pad("sink")
        .ok_or_else(|| SubError::new(codes::DECODE_FAILED, "the audio appsink has no sink pad"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(caps) = pad.current_caps() {
            return audio_format_from_caps(&caps, channels).map(Some);
        }
        if let Some(err) = pipeline_error(pipeline) {
            // A file with no audio at all fails as a pipeline error rather
            // than by quietly exposing no pad: decodebin cannot build a branch
            // that ends in the raw audio this decode asked for, so it gives up
            // — on a video-only file it reports the video decoder it could not
            // plug, and never reaches no-more-pads at all. As long as no audio
            // pad has been seen, that error says the file had no audio to give
            // this decode, not that decoding went wrong.
            if !saw_audio.load(Ordering::SeqCst) {
                return Ok(None);
            }
            return Err(err);
        }
        if sink.is_eos()
            || (no_more_pads.load(Ordering::SeqCst) && !saw_audio.load(Ordering::SeqCst))
        {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            return Err(SubError::new(
                codes::DECODE_TIMEOUT,
                "the audio stream did not negotiate within the frame budget",
            )
            .with_detail("timeout_ms", timeout.as_millis().to_string()));
        }
        std::thread::sleep(NEGOTIATION_SLICE);
    }
}

/// Reads a negotiated audio shape out of the caps the sink settled on.
fn audio_format_from_caps(
    caps: &gst::Caps,
    channels: AudioChannels,
) -> SubResult<(AudioFormat, Option<StereoDownmix>)> {
    let info = gstreamer_audio::AudioInfo::from_caps(caps)
        .map_err(|e| SubError::wrap(codes::DECODE_FAILED, "decoded caps are not audio", &e))?;
    let source_channels = u16::try_from(info.channels()).unwrap_or(u16::MAX);
    if info.rate() == 0 || source_channels == 0 || source_channels > MAX_CHANNELS {
        return Err(
            SubError::new(codes::UNSUPPORTED, "the audio stream has an unusable shape")
                .with_detail("sample_rate", info.rate().to_string())
                .with_detail("channels", source_channels.to_string()),
        );
    }
    let positions = info.positions();
    let roles = roles_from_positions(positions, source_channels);
    let source_layout = layout_from_roles(&roles);
    let downmix = match channels {
        AudioChannels::StereoDownmix => Some(StereoDownmix::from_roles(&roles)),
        AudioChannels::Source => None,
    };
    let format = AudioFormat {
        sample_rate: info.rate(),
        source_channels,
        source_layout,
        channels: if downmix.is_some() {
            2
        } else {
            source_channels
        },
    };
    tracing::info!(
        sample_rate = format.sample_rate,
        source_channels = format.source_channels,
        layout = format.source_layout.as_str(),
        channels = format.channels,
        "audio stream negotiated"
    );
    Ok((format, downmix))
}

/// Caps the audio sink accepts: interleaved 32-bit float, in system memory,
/// at whatever rate and channel count the source carries.
fn audio_sink_caps() -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .build()
}

/// How many decoded audio buffers the audio sink holds before it blocks. Wider
/// than the video queue because a caller usually pulls a run of audio between
/// frames.
const AUDIO_QUEUE_BUFFERS: u32 = 64;

/// How long the wait for the audio negotiation sleeps between checks.
const NEGOTIATION_SLICE: Duration = Duration::from_millis(2);

/// Caps the sink accepts: exactly one format, in system memory.
///
/// Naming a single format is what makes the negotiated output predictable. A
/// list would let the converter keep whichever format its upstream already
/// produces, so the same file would decode to different layouts depending on
/// which decoder was plugged.
fn sink_caps(format: FrameFormat) -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("format", format.as_str())
        .build()
}

/// How long one wait for a frame lasts before the bus is checked again.
const POLL_SLICE: Duration = Duration::from_millis(100);

/// Converts a `Duration` to a `ClockTime`, saturating rather than overflowing.
fn clock_time(duration: Duration) -> gst::ClockTime {
    u64::try_from(duration.as_nanos()).map_or(gst::ClockTime::MAX, gst::ClockTime::from_nseconds)
}

/// What [`Decoder::seek_to`] has to do to reach a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeekPlan {
    /// Keep decoding from where the pipeline already is.
    DecodeForward,
    /// Issue a flushing keyframe-backward seek first.
    Reseek,
}

/// Decides whether a target can be reached by decoding forward.
///
/// Only a target strictly ahead of the current position and no further than
/// `window` from it is decoded forward: anything behind the position needs the
/// pipeline rewound, and anything far ahead is cheaper to reach through a
/// keyframe seek than by decoding every picture in between.
fn plan_seek(
    position: Option<RationalTime>,
    target: RationalTime,
    window: RationalTime,
) -> SeekPlan {
    let Some(position) = position else {
        return SeekPlan::Reseek;
    };
    if target <= position {
        return SeekPlan::Reseek;
    }
    match target.checked_sub(position) {
        Some(ahead) if ahead <= window => SeekPlan::DecodeForward,
        _ => SeekPlan::Reseek,
    }
}

/// How far before its target a seek that overshot aims on its next attempt.
/// It doubles with each attempt, and the retries stop once the attempt reaches
/// the start of the stream or the backoff passes [`MAX_SEEK_BACKOFF`].
const SEEK_BACKOFF: Duration = Duration::from_millis(250);

/// The largest backoff a seek retries with. A container whose timestamps are
/// offset from its stream time by more than this is not something decoding
/// further forward can rescue.
const MAX_SEEK_BACKOFF: Duration = Duration::from_secs(8);

/// Where a seek that overshot should aim next, given where it last aimed, or
/// `None` when aiming earlier is pointless: the last attempt already reached
/// the start of the stream, or the backoff has grown past what any container
/// justifies.
fn earlier_target(aim: RationalTime, backoff: RationalTime) -> Option<RationalTime> {
    if aim.is_zero() || backoff > duration_time(MAX_SEEK_BACKOFF) {
        return None;
    }
    let earlier = aim.checked_sub(backoff)?;
    Some(if earlier.is_negative() {
        RationalTime::zero(NANOSECONDS)
    } else {
        earlier
    })
}

/// Exact nanosecond length; a `Duration` is a whole number of nanoseconds, so
/// nothing is rounded here.
fn duration_time(duration: Duration) -> RationalTime {
    nanoseconds(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
}

/// Exact nanosecond instant; GStreamer counts in nanoseconds, so this is a
/// change of unit rather than a conversion.
fn nanoseconds(value: u64) -> RationalTime {
    RationalTime::new(i64::try_from(value).unwrap_or(i64::MAX), NANOSECONDS)
}

/// Sends a video pad this decoder does not want to a fakesink, so the stream it
/// carries cannot stall the ones that are wanted.
fn discard_pad(pipeline: &gst::Pipeline, pad: &gst::Pad) {
    let Ok(sink) = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .property("async", false)
        .build()
    else {
        return;
    };
    if pipeline.add(&sink).is_err() || sink.sync_state_with_parent().is_err() {
        return;
    }
    if let Some(sink_pad) = sink.static_pad("sink") {
        let _ = pad.link(&sink_pad);
    }
}

/// Logs the decoder element decodebin plugs, and records its name.
fn watch_chosen_decoder(pipeline: &gst::Pipeline, chosen: &Arc<Mutex<Option<String>>>) {
    let chosen = Arc::clone(chosen);
    pipeline.connect_deep_element_added(move |_, _, element| {
        let Some(factory) = element.factory() else {
            return;
        };
        if !is_video_decoder(&factory) {
            return;
        }
        let name = factory.name().to_string();
        let hardware = is_hardware_decoder(&name);
        tracing::info!(decoder = %name, hardware, "video decoder plugged");
        if let Ok(mut slot) = chosen.lock() {
            *slot = Some(name);
        }
    });
}

/// Whether a factory decodes video, judged from its element class.
fn is_video_decoder(factory: &gst::ElementFactory) -> bool {
    let klass = factory.klass();
    klass.contains("Decoder") && klass.contains("Video")
}

/// Element-name prefixes of the hardware decoder families the plan names:
/// NVDEC on NVIDIA, VA-API on Linux, VideoToolbox on macOS and D3D12 on
/// Windows, plus the Intel and AMD vendor stacks that ship in the same
/// GStreamer builds.
const HARDWARE_DECODER_PREFIXES: &[&str] = &[
    "nvv4l2", "nvdec", "nvh", "nvav1", "nvmpeg", "nvvp", "va", "vtdec", "vtenc_", "d3d12", "d3d11",
    "qsv", "amfdec", "msdk",
];

/// Whether an element name belongs to one of the hardware decoder families.
///
/// Matching is on the element name because that is what the registry exposes
/// uniformly across platforms; `avdec_*` and the other software decoders match
/// nothing here and keep their own ranks.
fn is_hardware_decoder(name: &str) -> bool {
    // `vaapipostproc` and friends are not decoders, but this predicate is only
    // ever asked about factories that already classify as video decoders.
    HARDWARE_DECODER_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Rank every hardware video decoder above every software one, once per
/// process.
///
/// Autoplugging in decodebin sorts candidate factories by registry rank, so
/// raising the rank of the hardware families is what actually forces them
/// first. Software decoders keep their ranks and are still used when no
/// hardware decoder claims the stream.
pub(crate) fn prefer_hardware_decoders() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let promoted = promote_decoders(HARDWARE_DECODER_PREFIXES);
        if promoted.is_empty() {
            tracing::debug!("no hardware video decoder is installed; decoding in software");
        } else {
            tracing::info!(
                decoders = %promoted.join(", "),
                "hardware video decoders ranked ahead of software"
            );
        }
    });
}

/// Raises the registry rank of every video decoder whose element name starts
/// with one of `prefixes`, and returns the names it raised.
fn promote_decoders(prefixes: &[&str]) -> Vec<String> {
    let factories = gst::ElementFactory::factories_with_type(
        gst::ElementFactoryType::DECODER | gst::ElementFactoryType::MEDIA_VIDEO,
        gst::Rank::MARGINAL,
    );
    let mut promoted = Vec::new();
    for factory in factories {
        let name = factory.name().to_string();
        if !prefixes.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }
        factory.set_rank(gst::Rank::PRIMARY + HARDWARE_RANK_BOOST);
        promoted.push(name);
    }
    promoted
}

/// How far above `Rank::PRIMARY` a hardware decoder is ranked. The software
/// decoders that ship with GStreamer sit at or just above `PRIMARY`, so the
/// margin has to clear the small bumps distributions give their own.
const HARDWARE_RANK_BOOST: i32 = 64;

#[cfg(test)]
mod tests {
    use gstreamer::prelude::PluginFeatureExtManual;

    use super::{
        DecoderOptions, FrameFormat, HardwarePreference, RationalTime, gst, is_hardware_decoder,
        nanoseconds,
    };
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn hardware_decoder_names_are_recognised() {
        for name in [
            "vah264dec",
            "vah265dec",
            "vaav1dec",
            "nvh264dec",
            "nvh265sldec",
            "vtdec",
            "vtdec_hw",
            "d3d12h264dec",
            "d3d11h265dec",
            "qsvh264dec",
            "amfdech264",
            "msdkh264dec",
        ] {
            assert!(is_hardware_decoder(name), "{name} must count as hardware");
        }
    }

    #[test]
    fn software_decoder_names_are_not_hardware() {
        for name in [
            "avdec_h264",
            "avdec_h265",
            "openh264dec",
            "dav1ddec",
            "theoradec",
            "vp8dec",
        ] {
            assert!(!is_hardware_decoder(name), "{name} is software");
        }
    }

    #[test]
    fn ranking_a_decoder_family_makes_decodebin_plug_it() {
        gst::init().expect("GStreamer must initialise");
        // The mechanism is the one that forces nvdec, va, vtdec and d3d12 ahead
        // of software: a machine with no hardware decoder still proves it by
        // promoting a second software decoder past the installed default.
        let (Some(promoted), Some(default)) = (
            gst::ElementFactory::find("openh264dec"),
            gst::ElementFactory::find("avdec_h264"),
        ) else {
            eprintln!("skipping: this installation has only one H.264 decoder");
            return;
        };
        let before = promoted.rank();
        assert!(
            i32::from(before) <= i32::from(default.rank()),
            "the promoted decoder must start out no higher than the default"
        );

        let names = super::promote_decoders(&["openh264"]);
        assert!(
            names.iter().any(|name| name == "openh264dec"),
            "the promoted family is reported: {names:?}"
        );
        assert!(
            i32::from(promoted.rank()) > i32::from(default.rank()),
            "promotion must put the family ahead of the default decoder"
        );

        // And the rank is what autoplug follows: the promoted decoder, not the
        // default one, is the element that ends up in the pipeline.
        if let Some(path) = sub_test_support::try_fixture("bars_1080p_h264.mp4") {
            let mut decoder = super::Decoder::open_with(
                &path,
                DecoderOptions {
                    hardware: HardwarePreference::Software,
                    ..DecoderOptions::default()
                },
            )
            .unwrap_or_else(|e| panic!("[{}] {e}", e.code));
            let frame = decoder
                .next_frame()
                .unwrap_or_else(|e| panic!("[{}] {e}", e.code));
            assert!(frame.is_some(), "the fixture decodes");
            assert_eq!(
                decoder.decoder_element().as_deref(),
                Some("openh264dec"),
                "the highest-ranked decoder is the one that gets plugged"
            );
        } else {
            eprintln!("skipping the decode half: run scripts/gen-fixtures.sh");
        }

        promoted.set_rank(before);
    }

    #[test]
    fn a_missing_file_is_unreadable_before_any_pipeline_is_built() {
        let err = super::Decoder::open(&PathBuf::from("/definitely/not/here.mp4"))
            .expect_err("must fail");
        assert_eq!(err.code.as_str(), "media.file_unreadable");
        assert!(
            err.details.contains_key("path"),
            "the path is in the details"
        );
    }

    #[test]
    fn a_directory_cannot_be_decoded() {
        let err =
            super::Decoder::open(&std::env::temp_dir()).expect_err("a directory is not media");
        assert_eq!(err.code.as_str(), "media.file_unreadable");
    }

    #[test]
    fn default_options_prefer_hardware_with_a_bounded_frame_budget() {
        let options = DecoderOptions::default();
        assert_eq!(options.hardware, HardwarePreference::Prefer);
        assert_eq!(options.format, FrameFormat::Nv12);
        assert!(options.frame_timeout >= Duration::from_secs(1));
    }

    #[test]
    fn sink_caps_name_exactly_one_format() {
        gst::init().expect("GStreamer must initialise");
        for format in [FrameFormat::Nv12, FrameFormat::I420] {
            let caps = super::sink_caps(format);
            let structure = caps.structure(0).expect("one structure");
            assert_eq!(structure.name(), "video/x-raw");
            assert_eq!(
                structure.get::<String>("format").expect("a format field"),
                format.as_str()
            );
            assert_eq!(caps.size(), 1, "a single format keeps output predictable");
        }
    }

    #[test]
    fn frame_formats_describe_their_planes() {
        assert_eq!(FrameFormat::Nv12.as_str(), "NV12");
        assert_eq!(FrameFormat::Nv12.plane_count(), 2);
        assert_eq!(FrameFormat::I420.as_str(), "I420");
        assert_eq!(FrameFormat::I420.plane_count(), 3);
    }

    #[test]
    fn a_forward_target_inside_the_window_decodes_forward_instead_of_seeking() {
        let window = super::duration_time(Duration::from_secs(2));
        let position = nanoseconds(1_000_000_000);
        // Just ahead, and exactly at the edge of the window: no seek needed.
        for ahead in [1_i64, 40_000_000, 2_000_000_000] {
            let target = nanoseconds(u64::try_from(1_000_000_000 + ahead).expect("positive"));
            assert_eq!(
                super::plan_seek(Some(position), target, window),
                super::SeekPlan::DecodeForward,
                "{ahead} ns ahead is inside the GOP window"
            );
        }
    }

    #[test]
    fn a_backward_or_distant_target_re_seeks() {
        let window = super::duration_time(Duration::from_secs(2));
        let position = nanoseconds(1_000_000_000);
        for target in [
            nanoseconds(0),
            nanoseconds(999_999_999),
            // The position itself: the frame there has already been handed out,
            // so reaching it again means rewinding.
            nanoseconds(1_000_000_000),
            nanoseconds(3_000_000_001),
            nanoseconds(600_000_000_000),
        ] {
            assert_eq!(
                super::plan_seek(Some(position), target, window),
                super::SeekPlan::Reseek,
                "{} ns must re-seek",
                target.value()
            );
        }
    }

    #[test]
    fn the_first_seek_of_a_fresh_decoder_always_seeks() {
        let window = super::duration_time(Duration::from_secs(2));
        assert_eq!(
            super::plan_seek(None, nanoseconds(0), window),
            super::SeekPlan::Reseek,
            "nothing has been decoded, so there is nothing to decode forward from"
        );
    }

    #[test]
    fn a_target_at_a_frame_rate_is_compared_exactly_against_a_nanosecond_position() {
        // 29.97: frame 300 is 300 * 1001 / 30000 s, which is not a whole number
        // of nanoseconds. The comparison stays exact because it happens in
        // RationalTime, never in nanoseconds and never in floats.
        let rate = sub_time::Rational::new(30_000, 1001).expect("a valid rate");
        let target = RationalTime::from_frames(300, rate);
        let window = super::duration_time(Duration::from_secs(2));
        let just_before = nanoseconds(10_009_999_999);
        assert_eq!(
            super::plan_seek(Some(just_before), target, window),
            super::SeekPlan::DecodeForward,
            "the target is 10.01 s, which is still ahead of 10.009999999 s"
        );
        let just_after = nanoseconds(10_010_000_001);
        assert_eq!(
            super::plan_seek(Some(just_after), target, window),
            super::SeekPlan::Reseek
        );
    }

    #[test]
    fn a_seek_that_overshoots_aims_further_back_until_it_reaches_the_start() {
        let backoff = super::duration_time(Duration::from_millis(250));
        let aim = nanoseconds(3_040_000_000);
        let earlier = super::earlier_target(aim, backoff).expect("a later attempt");
        assert_eq!(earlier.value(), 2_790_000_000);
        // A backoff past the start of the stream clamps to it, and once there
        // no further attempt is made.
        let at_start = super::earlier_target(nanoseconds(100), backoff).expect("clamped");
        assert!(at_start.is_zero());
        assert_eq!(super::earlier_target(at_start, backoff), None);
    }

    #[test]
    fn the_seek_backoff_gives_up_rather_than_rewinding_for_ever() {
        let aim = nanoseconds(600_000_000_000);
        let too_far = super::duration_time(Duration::from_secs(9));
        assert_eq!(super::earlier_target(aim, too_far), None);
        let allowed = super::duration_time(Duration::from_secs(8));
        assert!(super::earlier_target(aim, allowed).is_some());
    }

    #[test]
    fn the_forward_window_is_an_exact_nanosecond_length() {
        let window = super::duration_time(Duration::from_millis(1500));
        assert_eq!(window.rate(), crate::probe::NANOSECONDS);
        assert_eq!(window.value(), 1_500_000_000);
        assert_eq!(
            super::duration_time(DecoderOptions::default().forward_decode_window).value(),
            2_000_000_000
        );
    }

    #[test]
    fn timestamps_are_exact_nanoseconds() {
        let pts = nanoseconds(1_001_000_000 / 30);
        assert_eq!(pts.rate(), crate::probe::NANOSECONDS);
        assert_eq!(pts.value(), 33_366_666);
    }
}
