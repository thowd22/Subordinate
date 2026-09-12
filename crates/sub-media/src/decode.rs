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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use gstreamer_video::{VideoFormat, VideoFrameExt, VideoInfo};
use sub_core::{ResultExt, SubError, SubResult};
use sub_model::MediaId;
use sub_time::{RationalTime, Rounding};

use crate::audio::{
    AudioBlock, AudioChannels, AudioFormat, MAX_CHANNELS, StereoDownmix, frames_at,
    layout_from_roles, roles_from_positions,
};
use crate::codes;
use crate::frame_cache::{FrameCache, FrameKey};
use crate::index::PtsIndex;
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
    /// Which of the file's audio streams is decoded, counted from zero in the
    /// order the container exposes them, which is the order
    /// [`crate::probe`] reports (TASK-153).
    ///
    /// A camera master or a mix-minus feed carries several audio streams and
    /// only one of them is the take a clip was cut with, so the choice is the
    /// caller's rather than always the first. Opening a stream the file does
    /// not carry fails with `media.no_audio_stream` rather than quietly
    /// falling back to another one. Ignored when no audio is decoded.
    pub audio_stream: u16,
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
    /// How many pictures beyond what a flushing seek would decode anyway a
    /// step will decode rather than flush the pipeline (TASK-133).
    ///
    /// A seek is not free even when it lands on the right keyframe: it drops
    /// what is in flight, makes the demuxer start again and makes the decoder
    /// build its reference state from scratch, which on the measured hardware
    /// decoders costs between ten and ninety milliseconds before a single
    /// picture exists. This is that cost expressed in the only currency the
    /// planner has, pictures: a target the decoder could reach by decoding at
    /// most this many pictures more than the seek itself would have decoded is
    /// reached by decoding forward. It only has meaning with a [`PtsIndex`],
    /// which is what can count pictures without decoding them.
    pub forward_decode_slack: u32,
    /// Byte budget for the pictures [`Decoder::seek_to`] keeps, so that a step
    /// back over ground the scrub has just covered is answered from memory
    /// rather than by a flushing seek (TASK-133).
    ///
    /// Dragging a playhead is not a walk in one direction: it moves forward a
    /// frame at a time and back a frame or two, and every one of those
    /// backward steps used to be a flush and a re-decode. The pictures a step
    /// pulls are kept here under a least-recently-used budget, so the back of
    /// that movement costs a lookup. Zero switches the cache off. Only
    /// [`Decoder::seek_to`] fills or reads it: playback goes through
    /// [`Decoder::next_frame`] and is left exactly as it was.
    pub gop_cache_bytes: usize,
}

/// Byte budget [`DecoderOptions::gop_cache_bytes`] uses by default.
///
/// 64 MiB is five 4K NV12 pictures or twenty-two at 1080p: the few frames
/// either side of the playhead that a hand's back-and-forth actually revisits,
/// and small enough that a project with several clips open does not pay for it
/// in gigabytes. The pictures are mapped GStreamer buffers, so the budget also
/// bounds how much of a decoder's buffer pool a scrub can hold.
pub const DEFAULT_GOP_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Pictures [`DecoderOptions::forward_decode_slack`] allows by default.
///
/// Twelve is the flush priced in pictures across the decoders this is measured
/// on: about fifty milliseconds of 4K software decode, about twenty of NVDEC.
/// It is deliberately of the same order as the cheapest flush measured (eleven
/// milliseconds on the box APU) rather than the dearest, so the rule never
/// trades a large decode for a small flush.
pub const DEFAULT_FORWARD_DECODE_SLACK: u32 = 12;

impl Default for DecoderOptions {
    fn default() -> Self {
        Self {
            streams: StreamSelection::Video,
            audio_channels: AudioChannels::StereoDownmix,
            audio_stream: 0,
            hardware: HardwarePreference::Prefer,
            format: FrameFormat::Nv12,
            frame_timeout: Duration::from_secs(10),
            forward_decode_window: Duration::from_secs(2),
            forward_decode_slack: DEFAULT_FORWARD_DECODE_SLACK,
            gop_cache_bytes: DEFAULT_GOP_CACHE_BYTES,
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

    /// Another handle on the same picture, or `None` when the buffer cannot be
    /// mapped a second time.
    ///
    /// No pixels are copied: a GStreamer buffer is reference counted and can
    /// carry more than one read-only mapping, so this is a reference and a map.
    /// It is what lets [`Decoder::seek_to`] both keep a picture in its cache
    /// and hand one to its caller (TASK-133), and what makes a cached picture
    /// cost a lookup rather than a decode.
    pub fn try_clone(&self) -> Option<Self> {
        let buffer = self.frame.buffer().to_owned();
        let info = self.frame.info().clone();
        let frame = gstreamer_video::VideoFrame::from_buffer_readable(buffer, &info).ok()?;
        Some(Self {
            frame,
            format: self.format,
            pts: self.pts,
            duration: self.duration,
        })
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

/// Where one [`Decoder::seek_to`] spent its time.
///
/// A scrub step is two very different costs stuck together: the flushing
/// keyframe seek, which tears down what is in flight and makes the demuxer and
/// the decoder start again at a keyframe, and the decode-forward from that
/// keyframe to the frame that was actually asked for. They respond to
/// different fixes, so the benchmark harness has to be able to tell them apart
/// (docs/PERFORMANCE.md); this is that split, measured around the production
/// path itself rather than guessed at from the outside.
///
/// Every field is an exact count, nanoseconds included: no timing value here
/// is a float.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SeekTiming {
    /// Nanoseconds spent issuing flushing seeks and waiting for the first
    /// frame each one produced. Zero when the target was reached by decoding
    /// forward inside the GOP the decoder was already in.
    pub seek_nanos: u64,
    /// Nanoseconds spent decoding the frames between there and the target,
    /// the target frame included.
    pub decode_forward_nanos: u64,
    /// Frames pulled out of the pipeline for this step, the target included.
    pub frames_decoded: u64,
    /// Flushing seeks issued, usually one and zero for a forward step inside
    /// the GOP. More than one means the first seek overshot its target and the
    /// backoff retry ran.
    pub seeks_issued: u64,
    /// One when the step was answered out of the decoder's picture cache and
    /// the pipeline was not touched at all, zero otherwise. A step that hits
    /// decodes nothing and seeks nothing, so both halves above are the lookup
    /// and the map alone.
    pub cache_hits: u64,
}

impl SeekTiming {
    /// The whole step: both halves added together.
    pub fn total_nanos(&self) -> u64 {
        self.seek_nanos.saturating_add(self.decode_forward_nanos)
    }

    /// Bills the span since `mark` to one half of the step and returns the new
    /// mark. `to_seek` charges the flushing seek, otherwise the decode-forward.
    fn charge(&mut self, mark: Instant, to_seek: bool) -> Instant {
        let now = Instant::now();
        let nanos =
            u64::try_from(now.saturating_duration_since(mark).as_nanos()).unwrap_or(u64::MAX);
        if to_seek {
            self.seek_nanos = self.seek_nanos.saturating_add(nanos);
        } else {
            self.decode_forward_nanos = self.decode_forward_nanos.saturating_add(nanos);
        }
        now
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
    last_seek: Option<SeekTiming>,
    index: Option<Arc<PtsIndex>>,
    /// Pictures [`Decoder::seek_to`] has already decoded, under a byte budget.
    /// The media ID is this decoder's own: the cache belongs to the handle, so
    /// nothing else can collide with its keys or evict its entries.
    cache: FrameCache<VideoFrame>,
    media: MediaId,
    /// How many pictures past what a seek would decode a step may decode
    /// instead of flushing.
    forward_slack: u32,
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
    #[allow(clippy::too_many_lines)] // Keep pipeline setup and startup error cleanup together.
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

        // A file with several streams of one type exposes several pads. The
        // first video stream is decoded; the audio stream is the one
        // `DecoderOptions::audio_stream` names, which is how a clip on a
        // multi-track camera master plays the track it was cut with
        // (TASK-153). Every other pad is discarded rather than left dangling.
        let saw_video = Arc::new(AtomicBool::new(false));
        let saw_audio = Arc::new(AtomicBool::new(false));
        link_decoded_pads(
            &source,
            &pipeline,
            &LinkTargets {
                video: video_sink
                    .as_ref()
                    .map(|sink| LinkTarget::new(sink, &saw_video, 0)),
                audio: audio_sink
                    .as_ref()
                    .map(|sink| LinkTarget::new(sink, &saw_audio, u32::from(options.audio_stream))),
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
            last_seek: None,
            index: None,
            cache: FrameCache::new(options.gop_cache_bytes),
            media: MediaId::new(),
            forward_slack: options.forward_decode_slack,
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
    /// on a long-GOP source the pictures between that keyframe and the one that
    /// was asked for still have to be decoded to build its reference chain, and
    /// the seek is followed by a decode forward that discards frames until one
    /// carries a presentation timestamp at or after `target`.
    ///
    /// A decoder driven by a [`PtsIndex`] does not have to *deliver* that chain
    /// (TASK-133): its seek is a flushing accurate seek whose segment opens
    /// just before the target, so the decoder clips those pictures instead of
    /// pushing them and they cost a decode and nothing else -- no colour
    /// conversion, no download out of the decoder's memory, and no copy into a
    /// [`VideoFrame`], which at 4K is megabytes a picture. The discard runs
    /// either way, so the frame this returns never depends on how faithfully a
    /// demuxer honours a segment. Every comparison here is exact [`RationalTime`] arithmetic, so
    /// a target expressed at a frame rate matches a nanosecond timestamp with
    /// no tolerance and no float.
    ///
    /// Most steps of a scrub need no seek at all, which is what makes dragging
    /// a playhead cheap (TASK-133):
    ///
    /// * A target **already decoded** is handed back out of this decoder's
    ///   picture cache. Nothing is seeked and nothing is decoded, which is what
    ///   a step back over ground the drag has just covered costs.
    /// * A target **ahead** of the position is reached by decoding on from the
    ///   last picture whenever that decodes no more pictures than the seek
    ///   would have decoded from its keyframe anyway, plus
    ///   [`DecoderOptions::forward_decode_slack`] for the flush itself. With no
    ///   index the rule is the cruder [`DecoderOptions::forward_decode_window`]
    ///   instead.
    ///
    /// [`Decoder::seek_count`] reports how many seeks were actually issued and
    /// [`Decoder::cache_stats`] how many steps the cache answered.
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
        let mut timing = SeekTiming::default();
        let seeks_before = self.seeks;
        let mut mark = Instant::now();
        // A picture this decoder has already handed out is the cheapest answer
        // there is: no seek, no decode and no flush, which is what makes the
        // backward half of dragging a playhead free (TASK-133).
        if let Some(frame) = self.cached_frame(target) {
            timing.cache_hits = 1;
            timing.charge(mark, false);
            self.last_seek = Some(timing);
            return Ok(Some(frame));
        }
        let (mut aim, mode) = self.seek_aim(target);
        if plan_seek(
            self.position,
            target,
            ForwardBudget {
                window: self.forward_window,
                slack: self.forward_slack,
            },
            self.index.as_deref(),
        ) == SeekPlan::Reseek
        {
            self.flushing_seek(aim, mode)?;
        }
        let mut backoff = duration_time(SEEK_BACKOFF);
        let mut retry_past_the_end = true;
        loop {
            let frame = self.next_frame();
            // The wait for a frame is charged to the seek when it is the first
            // frame after a flush -- that span carries the flush, the demuxer's
            // re-prime and the keyframe decode -- and to the decode-forward
            // otherwise. A frame that never arrived is still time spent, so the
            // error and end-of-stream paths bill it too.
            let first_after_seek = self.frames_since_seek <= 1;
            mark = timing.charge(mark, first_after_seek);
            // The attempt ended the stream without producing anything: either
            // the target really is past the last picture, or the seek opened
            // its segment past it, which is the same overshoot a late first
            // frame shows in the one case where no frame comes back to show it.
            // An index settles which; without one the two cannot be told apart,
            // so it is worth exactly one attempt further back and no more.
            //
            // A decoder that has never delivered a picture reports the end of
            // its stream as `media.no_video_stream` rather than as an end of
            // stream, which is the right answer for a file with no video in it
            // and the wrong one for a seek that landed past the end of a file
            // that has some: past the last picture there is no frame to return,
            // which is what `Ok(None)` says.
            let frame = match frame {
                Err(error)
                    if error.code == codes::NO_VIDEO_STREAM
                        && self.seeks > seeks_before
                        && self.saw_video.load(Ordering::SeqCst) =>
                {
                    Ok(None)
                }
                other => other,
            };
            let ended = matches!(frame, Ok(None));
            if ended
                && retry_past_the_end
                && self.seeks > seeks_before
                && self.frames_since_seek == 0
                && inside_the_file(self.index.as_deref(), target)
                && let Some(earlier) = earlier_target(aim, backoff)
            {
                retry_past_the_end = false;
                self.flushing_seek(earlier, mode)?;
                aim = earlier;
                continue;
            }
            let Some(frame) = frame? else {
                timing.seeks_issued = self.seeks - seeks_before;
                self.last_seek = Some(timing);
                return Ok(None);
            };
            timing.frames_decoded += 1;
            // Every picture this step pulls is kept, the ones before the
            // target included: they are exactly the ground a backward step is
            // about to ask for again.
            let frame = self.remember(frame);
            if frame.pts() < target {
                // Between the keyframe and the target: decoded only to build
                // the reference chain the target frame needs.
                continue;
            }
            // The first frame after a seek can be *past the frame the target
            // needed* even though a keyframe sits before it: a container whose
            // timestamps do not start at zero seeks in a stream time that is
            // offset from the presentation timestamps, so the demuxer opens its
            // segment at an instant that is not the one that was asked for. The
            // answer is to aim further back and decode forward from there.
            if self.frames_since_seek == 1
                && overshot(self.index.as_deref(), target, frame.pts())
                && let Some(earlier) = earlier_target(aim, backoff)
            {
                self.flushing_seek(earlier, mode)?;
                aim = earlier;
                backoff = backoff + backoff;
                continue;
            }
            timing.seeks_issued = self.seeks - seeks_before;
            self.last_seek = Some(timing);
            return Ok(Some(frame));
        }
    }

    /// The picture a step for `target` wants, if this decoder has already
    /// decoded it and still holds it.
    ///
    /// Only an index can answer: a target names an instant, and which picture
    /// covers that instant is exactly what an index knows and a cache keyed by
    /// presentation timestamp does not. Without one a step decodes, as it
    /// always did.
    ///
    /// A hit deliberately leaves the pipeline where it is. The position this
    /// decoder reports is where its *decoder* stands, not where the scrub was
    /// last looking, so the next forward step still decodes forward from the
    /// last picture the pipeline produced instead of flushing back to it.
    fn cached_frame(&mut self, target: RationalTime) -> Option<VideoFrame> {
        let wanted = self
            .index
            .as_deref()
            .and_then(|index| wanted_pts(index, target))?;
        let frame = self.cache.get(FrameKey::new(self.media, wanted))?;
        frame.try_clone()
    }

    /// Keeps a picture a step decoded and returns another handle on it.
    ///
    /// The cache holds one mapping and the caller gets the other; neither is a
    /// copy of the pixels. A budget of zero, or a buffer that cannot be mapped
    /// twice, hands the picture straight back and keeps nothing -- and so does
    /// a decoder with no index, which could never look a picture up again and
    /// would only be holding a decoder's buffers hostage.
    fn remember(&mut self, frame: VideoFrame) -> VideoFrame {
        if self.cache.budget() == 0 || self.index.is_none() {
            return frame;
        }
        let Some(copy) = frame.try_clone() else {
            return frame;
        };
        let key = FrameKey::new(self.media, frame.pts());
        self.cache.insert(key, frame);
        copy
    }

    /// What the picture cache has been doing: hits, misses and what it holds.
    ///
    /// A scrub that is working shows a hit for every step back over ground it
    /// has just covered.
    pub fn cache_stats(&self) -> crate::frame_cache::FrameCacheStats {
        self.cache.stats()
    }

    /// Drives seek planning from a PTS index of this file.
    ///
    /// The index turns the GOP guess [`DecoderOptions::forward_decode_window`]
    /// makes into knowledge: a forward step inside the GOP the decoder is
    /// already in decodes forward however far ahead it is instead of flushing
    /// the pipeline, a seek that must happen opens its segment just before the
    /// target instead of back at the keyframe -- so the pictures in between are
    /// decoded for their reference chain and clipped rather than delivered --
    /// and an overshoot is judged against the frame the index names rather than
    /// against the target. An index of some other file would only cost accuracy in speed,
    /// not in correctness -- the decode-forward still stops at the first frame
    /// at or after the target -- but there is no reason to hand one over.
    ///
    /// [`IndexedDecoder`](crate::IndexedDecoder) does this for its own index.
    pub fn set_index(&mut self, index: Arc<PtsIndex>) {
        self.index = Some(index);
    }

    /// How the last [`Decoder::seek_to`] split between the flushing seek and
    /// the decode-forward, or `None` before the first one.
    ///
    /// This is what the benchmark harness reports per scrub step; a caller
    /// tuning [`DecoderOptions::forward_decode_window`] can read it too.
    pub fn last_seek_timing(&self) -> Option<SeekTiming> {
        self.last_seek
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

    /// Where a seek meant to reach `target` aims, and how it names that
    /// instant.
    ///
    /// Without an index nothing is known about the file's timestamps, so the
    /// seek is the keyframe seek this decoder has always issued: the demuxer
    /// moves the segment back to the keyframe and every picture from there
    /// arrives, the ones before the target only to be thrown away.
    ///
    /// With one the seek can be accurate, which is what makes a scrub step
    /// cheap (TASK-133): the segment opens just before the target, so the run
    /// of pictures that only exists to build its reference chain is decoded and
    /// clipped by the decoder instead of being converted, downloaded and copied
    /// into a [`VideoFrame`] on its way to a caller that will discard it. Two
    /// things move the aim off the target itself:
    ///
    /// * It aims at the picture *before* the one the target names. A decoder
    ///   clips a buffer that straddles the start of the segment by trimming it
    ///   rather than dropping it, which on a file whose frame durations are not
    ///   its real ones (a variable-rate Matroska carries the same default
    ///   duration for every block) would hand back the earlier picture wearing
    ///   the target's timestamp. Opening the segment a picture early makes any
    ///   such trimmed buffer one this seek discards anyway.
    /// * It moves the instant into the timeline the container is seeked in. A
    ///   file whose first picture does not carry timestamp zero -- an encoder's
    ///   reordering delay puts the fixtures' first picture 80 ms in -- is
    ///   seeked in a timeline that starts at zero all the same.
    fn seek_aim(&self, target: RationalTime) -> (RationalTime, SeekMode) {
        let Some(aim) = self
            .index
            .as_deref()
            .and_then(|index| accurate_aim(index, target))
        else {
            return (target, SeekMode::Keyframe);
        };
        (aim, SeekMode::Accurate)
    }

    /// Issues the flushing seek a step that cannot decode forward needs.
    fn flushing_seek(&mut self, target: RationalTime, mode: SeekMode) -> SubResult<()> {
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
            .seek_simple(mode.flags(), gst::ClockTime::from_nseconds(nanos))
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
            if options.audio_stream == 0 {
                "the file carries no audio stream to decode"
            } else {
                "the file carries no audio stream at the index this decode asked for"
            },
        )
        .with_detail("audio_stream", options.audio_stream.to_string())),
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

/// One branch a decoded pad can be linked into: where it enters, whether it
/// has already claimed a stream, and which stream of its media type it wants.
struct LinkTarget<'a> {
    /// The branch's first element, whose sink pad a decoded pad links to.
    sink: &'a gst::Element,
    /// Set once this branch has claimed its stream.
    taken: &'a Arc<AtomicBool>,
    /// Which stream of this media type the branch takes, counted from zero in
    /// the order decodebin exposes them, which is the container's own order
    /// and so the order the probe reported (TASK-153).
    index: u32,
}

impl<'a> LinkTarget<'a> {
    fn new(sink: &'a gst::Element, taken: &'a Arc<AtomicBool>, index: u32) -> Self {
        Self { sink, taken, index }
    }
}

/// The branches a decoded pad can be linked into.
struct LinkTargets<'a> {
    video: Option<LinkTarget<'a>>,
    audio: Option<LinkTarget<'a>>,
}

/// A branch as the pad-added handler holds it, with the counter that says how
/// many streams of its media type have been exposed so far.
struct PendingBranch {
    /// The pad a decoded pad links to.
    entry: gst::Pad,
    /// Set once this branch has claimed its stream.
    taken: Arc<AtomicBool>,
    /// Which stream of this media type the branch takes.
    index: u32,
    /// How many streams of this media type have been offered so far.
    seen: AtomicU32,
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

    /// Resolves one branch into the form the pad-added handler holds.
    fn pending(target: &LinkTarget<'_>) -> SubResult<PendingBranch> {
        Ok(PendingBranch {
            entry: entry_pad(target.sink)?,
            taken: Arc::clone(target.taken),
            index: target.index,
            seen: AtomicU32::new(0),
        })
    }

    let video = targets.video.as_ref().map(pending).transpose()?;
    let audio = targets.audio.as_ref().map(pending).transpose()?;

    let weak_pipeline = pipeline.downgrade();
    source.connect_pad_added(move |_, pad| {
        let media = pad_media_type(pad);
        let target = match media.as_deref() {
            Some(name) if name.starts_with("video/") => video.as_ref(),
            Some(name) if name.starts_with("audio/") => audio.as_ref(),
            _ => None,
        };
        let Some(branch) = target else {
            if let Some(pipeline) = weak_pipeline.upgrade() {
                discard_pad(&pipeline, pad);
            }
            return;
        };
        // Every stream of this type is counted, so the one the branch asked
        // for is recognised by its position even though the pads arrive one
        // at a time; the others are disposed of rather than left dangling.
        let position = branch.seen.fetch_add(1, Ordering::SeqCst);
        if position != branch.index || branch.taken.swap(true, Ordering::SeqCst) {
            if let Some(pipeline) = weak_pipeline.upgrade() {
                discard_pad(&pipeline, pad);
            }
            return;
        }
        if let Err(err) = pad.link(&branch.entry) {
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
    /// Issue a flushing accurate seek at the target first. Where the keyframe
    /// it needs sits is the demuxer's business: the seek names the target so
    /// that the pictures before it stay outside the segment and are never
    /// pushed.
    Reseek,
}

/// What a step is allowed to decode rather than flush.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ForwardBudget {
    /// The time window used when nothing is known about where pictures are.
    window: RationalTime,
    /// Pictures a step may decode beyond what the seek would have decoded
    /// anyway; see [`DecoderOptions::forward_decode_slack`].
    slack: u32,
}

/// What each way of reaching a target costs in pictures decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ForwardCost {
    /// Pictures between the current position and the target.
    forward: usize,
    /// Pictures between the target's keyframe and the target, which a seek has
    /// to decode whatever else it does.
    after_seek: usize,
}

/// Prices both ways of reaching `target` from `position`, or `None` when the
/// index cannot place one of them: a target past the last picture, a position
/// that is not one of this file's timestamps, or no keyframe before the
/// target.
fn forward_cost(
    index: &PtsIndex,
    position: RationalTime,
    target: RationalTime,
) -> Option<ForwardCost> {
    let here = index.frame_at(position)?;
    let wanted = index.frame_at_or_after(target)?;
    let keyframe = index.keyframe_at_or_before(wanted)?;
    Some(ForwardCost {
        forward: wanted.saturating_sub(here),
        after_seek: wanted.saturating_sub(keyframe),
    })
}

/// Decides whether a target can be reached by decoding forward, and where a
/// seek that cannot should aim.
///
/// Without an index the rule is the time window: only a target strictly ahead
/// of the current position and no further than `window` from it is decoded
/// forward, because anything behind the position needs the pipeline rewound
/// and anything far ahead is usually cheaper to reach through a keyframe seek
/// than by decoding every picture in between. The window is a guess at a GOP
/// length, and it is wrong in both directions: on a source with GOPs longer
/// than the window it re-seeks inside the GOP the decoder is already in, and on
/// one with shorter GOPs it decodes through whole GOPs a seek would have
/// skipped.
///
/// With an index the guess is not needed (docs/PLAN.md §5.2, TASK-133), and
/// the two ways of reaching a target can be priced against each other in the
/// same currency. A decode-forward decodes the pictures between the position
/// and the target. A seek decodes the pictures between the target's keyframe
/// and the target -- it cannot start the decoder anywhere else -- and pays for
/// the flush on top, which is what [`DecoderOptions::forward_decode_slack`]
/// prices in pictures. So: decode forward whenever it decodes no more than the
/// seek would have, plus that slack.
///
/// A target inside the GOP the decoder is already in always wins that
/// comparison, because its keyframe is at or before the position; the earlier
/// rule is the special case this one generalises. What is new is the step that
/// lands a picture or two past the next keyframe, which used to flush the
/// pipeline to save a decode it was already cheaper than.
fn plan_seek(
    position: Option<RationalTime>,
    target: RationalTime,
    budget: ForwardBudget,
    index: Option<&PtsIndex>,
) -> SeekPlan {
    let Some(position) = position else {
        return SeekPlan::Reseek;
    };
    if target <= position {
        return SeekPlan::Reseek;
    }
    if let Some(cost) = index.and_then(|index| forward_cost(index, position, target)) {
        let allowed = cost.after_seek.saturating_add(budget.slack as usize);
        return if cost.forward <= allowed {
            SeekPlan::DecodeForward
        } else {
            SeekPlan::Reseek
        };
    }
    match target.checked_sub(position) {
        Some(ahead) if ahead <= budget.window => SeekPlan::DecodeForward,
        _ => SeekPlan::Reseek,
    }
}

/// How a flushing seek names the instant it wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeekMode {
    /// `KEY_UNIT` with `SNAP_BEFORE`: the demuxer moves the segment back to the
    /// keyframe at or before the instant asked for, so every picture from that
    /// keyframe onwards arrives. This is what a decoder with nothing but the
    /// container to go on has to ask for.
    Keyframe,
    /// `ACCURATE`: the segment opens where the seek names it and the demuxer
    /// still starts the decoder at the keyframe before it, so the pictures in
    /// between are decoded for their reference chain and clipped instead of
    /// being pushed. Only a seek aimed by an index asks for this; see
    /// [`Decoder::seek_aim`].
    Accurate,
}

impl SeekMode {
    /// The GStreamer flags. Both flush, so what is in flight is dropped and the
    /// frames that arrive next are the ones after the seek.
    fn flags(self) -> gst::SeekFlags {
        match self {
            Self::Keyframe => {
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT | gst::SeekFlags::SNAP_BEFORE
            }
            Self::Accurate => gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
        }
    }
}

/// Where an accurate seek for `target` aims, or `None` when the index cannot
/// answer and the keyframe seek has to be used instead.
///
/// The aim is the picture before the one the target names, moved into the
/// timeline the container is seeked in -- which is the presentation timestamps
/// shifted by the first picture's own. [`Decoder::seek_aim`] says why both
/// steps are there. Nothing is rounded: every instant here is exact
/// [`RationalTime`] arithmetic, and an aim that falls before the start of the
/// stream is the start of the stream.
fn accurate_aim(index: &PtsIndex, target: RationalTime) -> Option<RationalTime> {
    let offset = index.pts(0)?;
    let frame = index.frame_at_or_after(target)?;
    // A target that names the first picture of the file has nothing before it
    // to open the segment on: the start of the stream is it.
    let Some(aim) = frame.checked_sub(1).and_then(|before| index.pts(before)) else {
        return Some(RationalTime::zero(target.rate()));
    };
    match aim.checked_sub(offset) {
        Some(aim) if !aim.is_negative() => Some(aim),
        _ => Some(RationalTime::zero(target.rate())),
    }
}

/// Whether the first picture after a seek landed past the frame the target
/// needed, which is the one case a step has to aim further back and try again.
///
/// With an index this is exact: the index names the frame a target resolves to,
/// and anything later than that frame's timestamp is an overshoot. Without one
/// the only rule available is the target itself, which counts a target falling
/// between two pictures as an overshoot even though the picture that arrived is
/// the right one -- the accurate seek makes that rare, and the retry that
/// follows still lands on the same frame.
fn overshot(index: Option<&PtsIndex>, target: RationalTime, pts: RationalTime) -> bool {
    match index.and_then(|index| wanted_pts(index, target)) {
        Some(wanted) => pts > wanted,
        None => pts > target,
    }
}

/// Whether a target names a picture the file actually holds, as far as anything
/// known without decoding can say.
///
/// An index answers exactly. Without one there is nothing to answer with, so a
/// target is taken at its word: a seek that reached the end of the stream is
/// then given one more attempt further back rather than reported as the end of
/// the file.
fn inside_the_file(index: Option<&PtsIndex>, target: RationalTime) -> bool {
    index.is_none_or(|index| index.frame_at_or_after(target).is_some())
}

/// Timestamp of the frame a seek to `target` has to return, or `None` when the
/// index cannot answer.
fn wanted_pts(index: &PtsIndex, target: RationalTime) -> Option<RationalTime> {
    index.pts(index.frame_at_or_after(target)?)
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

    /// The planner's budget with the shipped slack, so a test that means to
    /// exercise the time window only has to name the window.
    fn budget(window: RationalTime) -> super::ForwardBudget {
        super::ForwardBudget {
            window,
            slack: super::DEFAULT_FORWARD_DECODE_SLACK,
        }
    }

    #[test]
    fn a_forward_target_inside_the_window_decodes_forward_instead_of_seeking() {
        let window = super::duration_time(Duration::from_secs(2));
        let position = nanoseconds(1_000_000_000);
        // Just ahead, and exactly at the edge of the window: no seek needed.
        for ahead in [1_i64, 40_000_000, 2_000_000_000] {
            let target = nanoseconds(u64::try_from(1_000_000_000 + ahead).expect("positive"));
            assert_eq!(
                super::plan_seek(Some(position), target, budget(window), None),
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
                super::plan_seek(Some(position), target, budget(window), None),
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
            super::plan_seek(None, nanoseconds(0), budget(window), None),
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
            super::plan_seek(Some(just_before), target, budget(window), None),
            super::SeekPlan::DecodeForward,
            "the target is 10.01 s, which is still ahead of 10.009999999 s"
        );
        let just_after = nanoseconds(10_010_000_001);
        assert_eq!(
            super::plan_seek(Some(just_after), target, budget(window), None),
            super::SeekPlan::Reseek,
            "10.010000001 s is already past the target, so it must rewind"
        );
    }

    /// Twenty-five pictures at 25 fps with a keyframe every five: one second
    /// of a five-frame-GOP source, which is what the planner reasons about.
    fn gop_index() -> crate::index::PtsIndex {
        let entries = (0..25)
            .map(|frame| crate::index::IndexEntry {
                pts_ns: frame * 40_000_000,
                keyframe: frame % 5 == 0,
            })
            .collect();
        crate::index::PtsIndex::from_entries(entries)
    }

    #[test]
    fn a_fresh_indexed_decoder_seeks_wherever_the_target_sits_in_its_gop() {
        let index = gop_index();
        let window = super::duration_time(Duration::from_secs(2));
        // Frame 13 sits in the GOP that starts at frame 10, which is 400 ms in;
        // the decoder is nowhere yet, so it has to seek whichever GOP that is.
        for target in [520_000_000_u64, 399_999_999, 400_000_000] {
            assert_eq!(
                super::plan_seek(None, nanoseconds(target), budget(window), Some(&index)),
                super::SeekPlan::Reseek,
                "nothing has been decoded, so {target} ns needs a seek"
            );
        }
    }

    /// The same twenty-five pictures, but starting 80 ms in, the way a file
    /// whose encoder reordered its output does.
    fn offset_gop_index() -> crate::index::PtsIndex {
        let entries = (0..25)
            .map(|frame| crate::index::IndexEntry {
                pts_ns: 80_000_000 + frame * 40_000_000,
                keyframe: frame % 5 == 0,
            })
            .collect();
        crate::index::PtsIndex::from_entries(entries)
    }

    #[test]
    fn an_accurate_seek_opens_its_segment_one_picture_before_the_target() {
        let index = gop_index();
        // Frame 13 is 520 ms in, so the segment opens on frame 12 at 480 ms:
        // a buffer the decoder trims to the start of the segment is then one
        // this seek discards anyway, never the target wearing its timestamp.
        let aim = super::accurate_aim(&index, nanoseconds(520_000_000)).expect("an aim");
        assert_eq!(aim.value(), 480_000_000);
        // A target between two pictures resolves to the later one, so the aim
        // is the earlier one.
        let between = super::accurate_aim(&index, nanoseconds(500_000_000)).expect("an aim");
        assert_eq!(between.value(), 480_000_000);
        // The first picture of the file has nothing before it.
        let first = super::accurate_aim(&index, nanoseconds(0)).expect("an aim");
        assert!(first.is_zero());
        // Past the last picture the index cannot answer, and the keyframe seek
        // has to be used instead.
        assert_eq!(
            super::accurate_aim(&index, nanoseconds(2_000_000_000)),
            None
        );
    }

    #[test]
    fn an_accurate_seek_names_its_instant_in_the_timeline_the_container_uses() {
        let index = offset_gop_index();
        // Frame 13 is now 600 ms in and frame 12 is 560 ms in, but the file is
        // seeked in a timeline that starts at zero: 80 ms earlier again.
        let aim = super::accurate_aim(&index, nanoseconds(600_000_000)).expect("an aim");
        assert_eq!(aim.value(), 480_000_000);
        // Nothing aims before the start of the stream.
        let second = super::accurate_aim(&index, nanoseconds(120_000_000)).expect("an aim");
        assert!(second.is_zero());
    }

    #[test]
    fn the_seek_flags_say_which_kind_of_seek_was_asked_for() {
        let keyframe = super::SeekMode::Keyframe.flags();
        assert!(keyframe.contains(gst::SeekFlags::FLUSH));
        assert!(keyframe.contains(gst::SeekFlags::KEY_UNIT | gst::SeekFlags::SNAP_BEFORE));
        let accurate = super::SeekMode::Accurate.flags();
        assert!(accurate.contains(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE));
        assert!(
            !accurate.contains(gst::SeekFlags::KEY_UNIT),
            "a key unit seek would move the segment back to the keyframe and \
             push every picture from there"
        );
    }

    #[test]
    fn only_an_index_can_say_whether_a_target_is_inside_the_file() {
        let index = gop_index();
        assert!(super::inside_the_file(
            Some(&index),
            nanoseconds(960_000_000)
        ));
        assert!(!super::inside_the_file(
            Some(&index),
            nanoseconds(960_000_001)
        ));
        assert!(
            super::inside_the_file(None, nanoseconds(600_000_000_000)),
            "without an index a target is taken at its word"
        );
    }

    #[test]
    fn an_overshoot_is_judged_against_the_frame_the_index_names() {
        let index = gop_index();
        // 520 ms falls between frame 12 (480 ms) and frame 13 (520 ms), so
        // frame 13 is the one the target resolves to.
        let target = nanoseconds(510_000_000);
        assert!(
            !super::overshot(Some(&index), target, nanoseconds(520_000_000)),
            "the frame the target needs is not an overshoot, late though it is"
        );
        assert!(
            super::overshot(Some(&index), target, nanoseconds(560_000_000)),
            "the picture after it is"
        );
        // Without an index the only rule left is the target itself, which is
        // what a decoder with no index has always used.
        assert!(super::overshot(None, target, nanoseconds(520_000_000)));
        assert!(!super::overshot(None, target, nanoseconds(510_000_000)));
        // Past the last indexed picture the index cannot answer either.
        let past_the_end = nanoseconds(2_000_000_000);
        assert!(super::overshot(
            Some(&index),
            past_the_end,
            nanoseconds(2_000_000_001)
        ));
    }

    #[test]
    fn an_indexed_forward_step_inside_the_gop_never_seeks() {
        let index = gop_index();
        // A window so narrow that the time rule would re-seek for anything.
        let window = super::duration_time(Duration::from_millis(1));
        let position = nanoseconds(440_000_000);
        for target in [480_000_000_u64, 520_000_000, 560_000_000] {
            assert_eq!(
                super::plan_seek(
                    Some(position),
                    nanoseconds(target),
                    budget(window),
                    Some(&index)
                ),
                super::SeekPlan::DecodeForward,
                "{target} ns is in the GOP the decoder is already inside"
            );
        }
    }

    #[test]
    fn an_indexed_step_just_past_the_next_keyframe_decodes_forward_anyway() {
        let index = gop_index();
        let window = super::duration_time(Duration::from_millis(1));
        // Frame 11 is where the decoder stands; frame 16 is in the next GOP.
        // Reaching it by decoding forward costs five pictures, where the seek
        // would decode one -- and pay for a flush, which the slack prices at
        // twelve. Decoding forward is the cheaper of the two.
        assert_eq!(
            super::plan_seek(
                Some(nanoseconds(440_000_000)),
                nanoseconds(640_000_000),
                budget(window),
                Some(&index)
            ),
            super::SeekPlan::DecodeForward,
            "five pictures is cheaper than a flush plus one"
        );
    }

    #[test]
    fn an_indexed_step_further_than_the_flush_is_worth_re_seeks() {
        let index = gop_index();
        let window = super::duration_time(Duration::from_secs(2));
        // Frame 1 to frame 24: twenty-three pictures forward, against the four
        // the seek decodes after its keyframe plus the twelve the flush is
        // priced at. The seek wins, so no step decodes the whole file to avoid
        // one.
        assert_eq!(
            super::plan_seek(
                Some(nanoseconds(40_000_000)),
                nanoseconds(960_000_000),
                budget(window),
                Some(&index)
            ),
            super::SeekPlan::Reseek,
            "a decode-forward that long is dearer than the flush it avoids"
        );
    }

    #[test]
    fn the_slack_is_what_decides_a_step_over_a_keyframe() {
        let index = gop_index();
        let window = super::duration_time(Duration::from_millis(1));
        // The same step as above, with no slack at all: the picture count
        // alone then says seek, which is what the rule did before the flush
        // had a price.
        let strict = super::ForwardBudget { window, slack: 0 };
        assert_eq!(
            super::plan_seek(
                Some(nanoseconds(440_000_000)),
                nanoseconds(640_000_000),
                strict,
                Some(&index)
            ),
            super::SeekPlan::Reseek
        );
    }

    #[test]
    fn the_planner_prices_both_ways_of_reaching_a_target_in_pictures() {
        let index = gop_index();
        // Frame 11 to frame 16: five pictures forward, one after the seek's
        // keyframe at frame 15.
        let cost = super::forward_cost(&index, nanoseconds(440_000_000), nanoseconds(640_000_000))
            .expect("both ends are in the index");
        assert_eq!(cost.forward, 5);
        assert_eq!(cost.after_seek, 1);
        // A target past the last picture cannot be priced.
        assert_eq!(
            super::forward_cost(&index, nanoseconds(440_000_000), nanoseconds(2_000_000_000)),
            None
        );
    }

    #[test]
    fn an_index_that_cannot_answer_falls_back_to_the_time_window() {
        let index = gop_index();
        let window = super::duration_time(Duration::from_secs(2));
        // Past the last indexed picture: nothing to learn, so the window rule
        // decides.
        let past_the_end = nanoseconds(2_000_000_000);
        assert_eq!(
            super::plan_seek(None, past_the_end, budget(window), Some(&index)),
            super::SeekPlan::Reseek
        );
        assert_eq!(
            super::plan_seek(
                Some(nanoseconds(1_999_000_000)),
                past_the_end,
                budget(window),
                Some(&index)
            ),
            super::SeekPlan::DecodeForward
        );
    }

    #[test]
    fn a_backward_target_re_seeks_even_inside_the_current_gop() {
        let index = gop_index();
        let window = super::duration_time(Duration::from_secs(2));
        // The pipeline cannot run backwards, so a target behind the position is
        // a seek however close it is.
        assert_eq!(
            super::plan_seek(
                Some(nanoseconds(440_000_000)),
                nanoseconds(400_000_000),
                budget(window),
                Some(&index)
            ),
            super::SeekPlan::Reseek
        );
    }

    #[test]
    fn seek_timing_charges_each_half_of_a_step_separately() {
        // A coarse platform clock (Windows in particular) can report a zero
        // elapsed time across two adjacent reads, so wait for it to tick
        // rather than assume any work takes a measurable moment.
        fn after_a_tick(mark: std::time::Instant) -> std::time::Instant {
            loop {
                let now = std::time::Instant::now();
                if now.saturating_duration_since(mark).as_nanos() > 0 {
                    return mark;
                }
                std::hint::spin_loop();
            }
        }

        let mut timing = super::SeekTiming::default();
        let mark = std::time::Instant::now();
        let mark = timing.charge(after_a_tick(mark), true);
        let _ = timing.charge(after_a_tick(mark), false);
        assert!(timing.seek_nanos > 0, "the seek half was billed");
        assert!(timing.decode_forward_nanos > 0, "so was the other half");
        assert_eq!(
            timing.total_nanos(),
            timing.seek_nanos + timing.decode_forward_nanos
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
