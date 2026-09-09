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
use sub_time::RationalTime;

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
    /// Whether hardware decoders are preferred.
    pub hardware: HardwarePreference,
    /// Pixel format frames are delivered in. NV12 is what the compositor
    /// uploads; I420 is the documented fallback for a caller that would rather
    /// take a software decoder's own planar output than have it converted.
    pub format: FrameFormat,
    /// How long [`Decoder::next_frame`] waits for a frame before giving up with
    /// `media.decode_timeout`. It bounds a stalled pipeline, not the whole
    /// decode: the budget applies to each frame.
    pub frame_timeout: Duration,
}

impl Default for DecoderOptions {
    fn default() -> Self {
        Self {
            hardware: HardwarePreference::Prefer,
            format: FrameFormat::Nv12,
            frame_timeout: Duration::from_secs(10),
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
    sink: AppSink,
    chosen: Arc<Mutex<Option<String>>>,
    frame_timeout: Duration,
    saw_video: Arc<AtomicBool>,
    delivered: u64,
    finished: bool,
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

        let metadata = std::fs::metadata(path)
            .sub_context_with(codes::FILE_UNREADABLE, || "media file cannot be read")
            .map_err(|e| e.with_detail("path", path.display().to_string()))?;
        if !metadata.is_file() {
            return Err(
                SubError::new(codes::FILE_UNREADABLE, "media path is not a file")
                    .with_detail("path", path.display().to_string()),
            );
        }
        let uri = gst::glib::filename_to_uri(path, None)
            .map_err(|e| SubError::wrap(codes::FILE_UNREADABLE, "media path is not a URI", &e))
            .map_err(|e| e.with_detail("path", path.display().to_string()))?;

        if options.hardware == HardwarePreference::Prefer {
            prefer_hardware_decoders();
        }

        let pipeline = gst::Pipeline::new();
        let source = gst::ElementFactory::make("uridecodebin")
            .property("uri", &uri)
            // Only raw video is exposed; decodebin disposes of the other
            // streams itself, so no pad is left unlinked to stall the flow.
            .property("expose-all-streams", false)
            .property("caps", raw_video_caps())
            .build()
            .sub_context(codes::UNSUPPORTED, "uridecodebin is unavailable")?;
        let convert = gst::ElementFactory::make("videoconvert")
            .build()
            .sub_context(codes::UNSUPPORTED, "videoconvert is unavailable")?;
        let sink = gst::ElementFactory::make("appsink")
            .property("sync", false)
            .property("max-buffers", 2_u32)
            .property("caps", sink_caps(options.format))
            .build()
            .sub_context(codes::UNSUPPORTED, "appsink is unavailable")?;

        pipeline
            .add_many([&source, &convert, &sink])
            .sub_context(codes::DECODE_FAILED, "could not build the decode pipeline")?;
        convert
            .link(&sink)
            .sub_context(codes::DECODE_FAILED, "could not link the decode pipeline")?;

        let convert_sink_pad = convert
            .static_pad("sink")
            .ok_or_else(|| SubError::new(codes::DECODE_FAILED, "videoconvert has no sink pad"))?;
        // A file with several video streams exposes several pads; the first one
        // is decoded and any later one is discarded rather than left dangling.
        let saw_video = Arc::new(AtomicBool::new(false));
        let taken = Arc::clone(&saw_video);
        let weak_pipeline = pipeline.downgrade();
        source.connect_pad_added(move |_, pad| {
            if taken.swap(true, Ordering::SeqCst) {
                if let Some(pipeline) = weak_pipeline.upgrade() {
                    discard_pad(&pipeline, pad);
                }
                return;
            }
            if let Err(err) = pad.link(&convert_sink_pad) {
                tracing::warn!(%err, "decoded video pad could not be linked");
            }
        });

        let chosen = Arc::new(Mutex::new(None));
        watch_chosen_decoder(&pipeline, &chosen);

        let decoder = Self {
            sink: sink
                .downcast::<AppSink>()
                .map_err(|_| SubError::new(codes::DECODE_FAILED, "appsink has the wrong type"))?,
            pipeline,
            chosen,
            frame_timeout: options.frame_timeout,
            saw_video,
            delivered: 0,
            finished: false,
        };
        decoder
            .pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| {
                SubError::wrap(
                    codes::DECODE_FAILED,
                    "the decode pipeline would not start",
                    &e,
                )
                .with_detail("path", path.display().to_string())
            })?;
        Ok(decoder)
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
    /// and `media.no_video_stream` when the file ends without a single frame.
    pub fn next_frame(&mut self) -> SubResult<Option<VideoFrame>> {
        if self.finished {
            return Ok(None);
        }
        let deadline = Instant::now() + self.frame_timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            // Waiting for the whole budget in one call would hide an error the
            // pipeline has already posted, so the wait is sliced and the bus is
            // read between slices.
            if let Some(sample) = self.sink.try_pull_sample(clock_time(left.min(POLL_SLICE))) {
                let frame = Self::frame_from_sample(&sample)?;
                self.delivered += 1;
                return Ok(Some(frame));
            }
            if let Some(err) = self.pipeline_error() {
                self.finished = true;
                // A file that never exposed a video pad usually fails as a
                // pipeline error; "no video stream" says far more than that.
                return Err(self.no_video_stream().unwrap_or(err));
            }
            if self.sink.is_eos() {
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
        let bus = self.pipeline.bus()?;
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
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder")
            .field("decoder_element", &self.decoder_element())
            .field("frames_delivered", &self.delivered)
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

/// Caps decodebin treats as decoded: any raw video, including the memory
/// features a hardware decoder attaches.
fn raw_video_caps() -> gst::Caps {
    let mut caps = gst::Caps::builder("video/x-raw").build();
    if let Some(caps) = caps.get_mut() {
        caps.set_features(0, Some(gst::CapsFeatures::new_any()));
    }
    caps
}

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
fn prefer_hardware_decoders() {
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
        DecoderOptions, FrameFormat, HardwarePreference, gst, is_hardware_decoder, nanoseconds,
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
    fn timestamps_are_exact_nanoseconds() {
        let pts = nanoseconds(1_001_000_000 / 30);
        assert_eq!(pts.rate(), crate::probe::NANOSECONDS);
        assert_eq!(pts.value(), 33_366_666);
    }
}
