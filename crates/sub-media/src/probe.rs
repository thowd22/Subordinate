//! Probing a media file into a [`MediaInfo`] with the GStreamer discoverer
//! (docs/PLAN.md §5.2).
//!
//! Import needs the shape of a file before anything is decoded: how long it is,
//! what streams it holds, what rate the pictures come at, whether that rate is
//! constant, how the frames are rotated and what colour they were tagged with.
//! [`probe`] answers all of that in one pass and never guesses with floats:
//! durations are [`RationalTime`] and rates are [`Rational`].
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! let info = sub_media::probe(std::path::Path::new("/media/a.mp4"))?;
//! println!("{} for {:?}", info.container, info.duration);
//! # Ok(())
//! # }
//! ```

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_pbutils::prelude::*;
use gstreamer_pbutils::{Discoverer, DiscovererAudioInfo, DiscovererResult, DiscovererVideoInfo};
use gstreamer_video::{
    VideoColorMatrix, VideoColorPrimaries, VideoColorimetry, VideoTransferFunction,
};
use sub_core::{ResultExt, SubError, SubResult};
use sub_model::sequence::{ColorPrimaries, ColorSpace, ColorTags, TransferFunction};
use sub_time::{Rational, RationalTime};

use crate::codes;

/// The rate durations are reported at: one unit per nanosecond, which is the
/// unit GStreamer itself measures in, so no rounding happens on the way in.
/// Callers rescale to a sequence rate when they need frames.
pub const NANOSECONDS: Rational = match Rational::new(1_000_000_000, 1) {
    Some(rate) => rate,
    None => unreachable!(),
};

/// How the frames of a video stream are spaced in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FrameTiming {
    /// Every frame lasts the same length: the nominal frame rate describes the
    /// whole stream.
    Constant,
    /// Frame durations genuinely vary: the nominal frame rate is only the peak,
    /// and seeking needs the PTS index of TASK-16.
    Variable,
    /// Not established, because the scan was disabled, ran out of time, or the
    /// stream carried too few timestamps to judge.
    #[default]
    Unknown,
}

impl FrameTiming {
    /// True only when the scan positively found varying frame durations.
    pub fn is_variable(self) -> bool {
        self == Self::Variable
    }
}

/// The rotation a video stream is tagged with, clockwise, as read from the
/// container's `image-orientation` tag.
///
/// The MVP stores the tag; applying it is the compositor's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    /// No rotation, the default when a file carries no orientation tag.
    #[default]
    None,
    /// A quarter turn clockwise.
    Cw90,
    /// A half turn.
    Cw180,
    /// A quarter turn anticlockwise.
    Cw270,
}

impl Rotation {
    /// Clockwise rotation in degrees: 0, 90, 180 or 270.
    pub fn degrees(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Cw90 => 90,
            Self::Cw180 => 180,
            Self::Cw270 => 270,
        }
    }

    /// Parses an `image-orientation` tag value, returning the rotation and
    /// whether the picture is also mirrored horizontally.
    ///
    /// Unknown values are reported as no rotation, matching GStreamer's own
    /// treatment of an unparseable orientation.
    fn from_tag(value: &str) -> (Self, bool) {
        let (mirrored, rotation) = value
            .strip_prefix("flip-")
            .map_or((false, value), |rest| (true, rest));
        let rotation = match rotation {
            "rotate-90" => Self::Cw90,
            "rotate-180" => Self::Cw180,
            "rotate-270" => Self::Cw270,
            _ => Self::None,
        };
        (rotation, mirrored)
    }
}

/// One video stream as the probe found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoStreamInfo {
    /// Media type of the coded stream, for example `video/x-h264`.
    pub codec: String,
    /// Human-readable codec name from GStreamer, for example `H.264 / AVC`.
    pub codec_description: String,
    /// Coded width in pixels.
    pub width: u32,
    /// Coded height in pixels.
    pub height: u32,
    /// Nominal frame rate as an exact rational, when the file declares one.
    /// For a variable-frame-rate source this is the rate the container claims,
    /// which is usually the peak rate rather than one every frame follows.
    pub frame_rate: Option<Rational>,
    /// Pixel (sample) aspect ratio; `1/1` for square pixels.
    pub sample_aspect: Rational,
    /// Rotation tag, clockwise.
    pub rotation: Rotation,
    /// True when the orientation tag also mirrors the picture horizontally.
    pub mirrored: bool,
    /// Colour tags read from the container or bitstream (decision-3).
    pub color: ColorTags,
    /// True when the stream is tagged interlaced.
    pub interlaced: bool,
    /// What the frame-timing scan concluded about frame spacing.
    pub timing: FrameTiming,
}

/// One audio stream as the probe found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStreamInfo {
    /// Media type of the coded stream, for example `audio/mpeg`.
    pub codec: String,
    /// Human-readable codec name from GStreamer.
    pub codec_description: String,
    /// Channel count.
    pub channels: u16,
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Language tag, when the container carries one.
    pub language: Option<String>,
}

/// Everything the probe learned about a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaInfo {
    /// Media type of the container, for example `video/quicktime`. An
    /// elementary stream with no container reports its own media type.
    pub container: String,
    /// The container's human-readable name from its `container-format` tag,
    /// for example `ISO MP4/M4A`, when it carries one.
    pub container_format: Option<String>,
    /// Total duration, exact in nanoseconds, when the file declares one.
    pub duration: Option<RationalTime>,
    /// Whether the source can be seeked; a stream that cannot is playback-only.
    pub seekable: bool,
    /// Video streams, in file order.
    pub video: Vec<VideoStreamInfo>,
    /// Audio streams, in file order.
    pub audio: Vec<AudioStreamInfo>,
}

impl MediaInfo {
    /// True when the file carries at least one video stream.
    pub fn has_video(&self) -> bool {
        !self.video.is_empty()
    }

    /// True when the file carries at least one audio stream.
    pub fn has_audio(&self) -> bool {
        !self.audio.is_empty()
    }

    /// True when any video stream was positively found to be
    /// variable-frame-rate.
    pub fn is_variable_frame_rate(&self) -> bool {
        self.video.iter().any(|v| v.timing.is_variable())
    }
}

/// How a probe should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeOptions {
    /// Total budget for the probe. The discoverer gets it, and the
    /// frame-timing scan gets whatever is left. GStreamer's discoverer refuses
    /// a timeout under a second, so a shorter budget still lets it run for one
    /// second before the scan is asked for whatever time remains.
    pub timeout: Duration,
    /// Whether to scan frame timestamps to classify frame timing. The scan
    /// parses (but never decodes) the whole video stream, so it costs a read of
    /// the file; without it every stream reports [`FrameTiming::Unknown`].
    pub scan_frame_timing: bool,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            scan_frame_timing: true,
        }
    }
}

/// The shortest timeout GStreamer's discoverer accepts.
const MIN_DISCOVERER_TIMEOUT: Duration = Duration::from_secs(1);

/// Probes `path` with the default options.
///
/// # Errors
///
/// Returns `media.file_unreadable` when the path is not a readable file,
/// `media.unsupported` when this GStreamer installation cannot handle the
/// format, `media.probe_timeout` when the budget runs out and
/// `media.probe_failed` for a corrupt or otherwise unreadable file.
pub fn probe(path: &Path) -> SubResult<MediaInfo> {
    probe_with(path, ProbeOptions::default())
}

/// Probes `path`, returning what the GStreamer discoverer found.
///
/// # Errors
///
/// As [`probe`].
pub fn probe_with(path: &Path, options: ProbeOptions) -> SubResult<MediaInfo> {
    let started = Instant::now();
    gst::init().map_err(|e| SubError::wrap(codes::INIT_FAILED, "GStreamer failed to start", &e))?;

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

    let timeout = clock_time(options.timeout.max(MIN_DISCOVERER_TIMEOUT));
    let discoverer = Discoverer::new(timeout)
        .map_err(|e| SubError::wrap(codes::INIT_FAILED, "discoverer unavailable", &e))?;
    let info = discoverer
        .discover_uri(&uri)
        .map_err(|e| discoverer_error(&e, path))?;
    check_result(info.result(), path)?;

    let (container, container_format) = container_of(&info);
    let mut media = MediaInfo {
        container,
        container_format,
        duration: info.duration().map(|d| duration_of(d.nseconds())),
        seekable: info.is_seekable(),
        video: info.video_streams().iter().map(video_stream).collect(),
        audio: info.audio_streams().iter().map(audio_stream).collect(),
    };

    if options.scan_frame_timing && media.has_video() {
        let left = options.timeout.saturating_sub(started.elapsed());
        let timing = scan_frame_timing(path, left);
        for stream in &mut media.video {
            stream.timing = timing;
        }
    }

    Ok(media)
}

/// Converts a `Duration` to a `ClockTime`, saturating at the maximum GStreamer
/// can express rather than overflowing.
fn clock_time(duration: Duration) -> gst::ClockTime {
    u64::try_from(duration.as_nanos()).map_or(gst::ClockTime::MAX, gst::ClockTime::from_nseconds)
}

/// Exact nanosecond duration; GStreamer counts in nanoseconds, so this is a
/// change of unit, not a conversion.
fn duration_of(nanoseconds: u64) -> RationalTime {
    RationalTime::new(i64::try_from(nanoseconds).unwrap_or(i64::MAX), NANOSECONDS)
}

/// Maps a discoverer failure onto a stable error code.
fn discoverer_error(err: &gst::glib::Error, path: &Path) -> SubError {
    let code = if err.matches(gst::CoreError::MissingPlugin)
        || err.matches(gst::StreamError::CodecNotFound)
        || err.matches(gst::StreamError::TypeNotFound)
        || err.matches(gst::StreamError::WrongType)
    {
        codes::UNSUPPORTED
    } else if err.matches(gst::ResourceError::NotFound)
        || err.matches(gst::ResourceError::OpenRead)
        || err.matches(gst::ResourceError::Read)
    {
        codes::FILE_UNREADABLE
    } else {
        codes::PROBE_FAILED
    };
    SubError::wrap(code, "media file could not be probed", err)
        .with_detail("path", path.display().to_string())
}

/// Turns a non-`Ok` discoverer result into an error; the discoverer reports
/// some outcomes (a timeout, most notably) through the result rather than
/// through a `GError`.
fn check_result(result: DiscovererResult, path: &Path) -> SubResult<()> {
    let (code, message) = match result {
        DiscovererResult::Ok => return Ok(()),
        DiscovererResult::Timeout => (codes::PROBE_TIMEOUT, "probing the media file timed out"),
        DiscovererResult::MissingPlugins => (
            codes::UNSUPPORTED,
            "no GStreamer plugin handles this media file",
        ),
        DiscovererResult::UriInvalid => (codes::FILE_UNREADABLE, "media URI is not valid"),
        DiscovererResult::Busy => (codes::PROBE_FAILED, "the discoverer is busy"),
        _ => (codes::PROBE_FAILED, "media file could not be probed"),
    };
    Err(SubError::new(code, message)
        .with_detail("path", path.display().to_string())
        .with_detail("result", format!("{result:?}")))
}

/// The container media type and its human-readable name.
fn container_of(info: &gstreamer_pbutils::DiscovererInfo) -> (String, Option<String>) {
    let container = info
        .stream_info()
        .and_then(|stream| media_type(stream.caps().as_ref()))
        .unwrap_or_else(|| "unknown".to_owned());
    let format = info.tags().and_then(|tags| {
        tags.get::<gst::tags::ContainerFormat>()
            .map(|v| v.get().to_owned())
    });
    (container, format)
}

/// The media type name of the first structure of `caps`.
fn media_type(caps: Option<&gst::Caps>) -> Option<String> {
    caps?.structure(0).map(|s| s.name().to_string())
}

/// A human-readable codec name, falling back to the media type.
fn codec_description(caps: Option<&gst::Caps>) -> String {
    caps.map_or_else(
        || "unknown".to_owned(),
        |caps| gstreamer_pbutils::pb_utils_get_codec_description(caps).to_string(),
    )
}

/// Reads one video stream out of the discoverer's report.
fn video_stream(info: &DiscovererVideoInfo) -> VideoStreamInfo {
    let caps = info.caps();
    let structure = caps.as_ref().and_then(|c| c.structure(0));
    let (rotation, mirrored) = info
        .tags()
        .and_then(|tags| {
            tags.get::<gst::tags::ImageOrientation>()
                .map(|v| Rotation::from_tag(v.get()))
        })
        .unwrap_or((Rotation::None, false));

    VideoStreamInfo {
        codec: media_type(caps.as_ref()).unwrap_or_else(|| "unknown".to_owned()),
        codec_description: codec_description(caps.as_ref()),
        width: info.width(),
        height: info.height(),
        frame_rate: rational(info.framerate()),
        sample_aspect: rational(info.par()).unwrap_or(Rational::ONE),
        rotation,
        mirrored,
        color: structure
            .and_then(|s| s.get::<String>("colorimetry").ok())
            .map_or_else(ColorTags::default, |c| color_tags(&c)),
        interlaced: info.is_interlaced(),
        timing: FrameTiming::Unknown,
    }
}

/// Reads one audio stream out of the discoverer's report.
fn audio_stream(info: &DiscovererAudioInfo) -> AudioStreamInfo {
    let caps = info.caps();
    AudioStreamInfo {
        codec: media_type(caps.as_ref()).unwrap_or_else(|| "unknown".to_owned()),
        codec_description: codec_description(caps.as_ref()),
        channels: u16::try_from(info.channels()).unwrap_or(u16::MAX),
        sample_rate: info.sample_rate(),
        language: info.language().map(|l| l.to_string()),
    }
}

/// Converts a GStreamer fraction to an exact [`Rational`], rejecting the
/// zero and negative values GStreamer uses for "unknown".
fn rational(fraction: gst::Fraction) -> Option<Rational> {
    let numerator = u32::try_from(fraction.numer()).ok()?;
    let denominator = u32::try_from(fraction.denom()).ok()?;
    Rational::new(numerator, denominator)
}

/// Maps a GStreamer colorimetry string onto the model's colour tags.
///
/// An unparseable or absent value maps to [`ColorTags::default`], which is the
/// Rec.709 the MVP renderer assumes, with unknowns kept as `Unknown` so nothing
/// pretends to know more than the file said.
fn color_tags(colorimetry: &str) -> ColorTags {
    let Ok(parsed) = colorimetry.parse::<VideoColorimetry>() else {
        return ColorTags::default();
    };
    ColorTags {
        space: match parsed.matrix() {
            VideoColorMatrix::Bt709 => ColorSpace::Rec709,
            VideoColorMatrix::Bt601 => ColorSpace::Rec601,
            VideoColorMatrix::Bt2020 => ColorSpace::Rec2020,
            _ => ColorSpace::Unknown,
        },
        transfer: match parsed.transfer() {
            VideoTransferFunction::Bt709 | VideoTransferFunction::Bt202010 => {
                TransferFunction::Bt709
            }
            VideoTransferFunction::Srgb => TransferFunction::Srgb,
            VideoTransferFunction::Smpte2084 => TransferFunction::Pq,
            VideoTransferFunction::AribStdB67 => TransferFunction::Hlg,
            _ => TransferFunction::Unknown,
        },
        primaries: match parsed.primaries() {
            VideoColorPrimaries::Bt709 => ColorPrimaries::Bt709,
            VideoColorPrimaries::Bt470bg | VideoColorPrimaries::Smpte170m => ColorPrimaries::Bt601,
            VideoColorPrimaries::Bt2020 => ColorPrimaries::Bt2020,
            VideoColorPrimaries::Smpterp431 | VideoColorPrimaries::Smpteeg432 => {
                ColorPrimaries::DciP3
            }
            _ => ColorPrimaries::Unknown,
        },
    }
}

/// How far two frame durations may differ before the stream counts as
/// variable-frame-rate: five per cent, comfortably above the one-nanosecond
/// wobble of a 1001-denominator rate and far below a real rate change.
const TIMING_TOLERANCE_NUM: u128 = 105;
/// Denominator of [`TIMING_TOLERANCE_NUM`].
const TIMING_TOLERANCE_DEN: u128 = 100;

/// Parses (never decodes) the file and classifies its frame spacing.
///
/// The scan runs `filesrc ! parsebin`, collects the presentation timestamps of
/// the video buffers and compares the shortest frame duration with the longest.
/// Anything that goes wrong — a missing parser, a budget that runs out, a file
/// with too few timestamps — is reported as [`FrameTiming::Unknown`] rather than
/// as an error: the file has already been probed successfully by then.
fn scan_frame_timing(path: &Path, budget: Duration) -> FrameTiming {
    if budget.is_zero() {
        return FrameTiming::Unknown;
    }
    match collect_video_pts(path, budget) {
        Ok(mut timestamps) => classify_timing(&mut timestamps),
        Err(err) => {
            tracing::debug!(
                path = %path.display(),
                error = %err,
                "frame timing scan did not run; timing stays unknown"
            );
            FrameTiming::Unknown
        }
    }
}

/// Runs the parse-only pipeline and returns every video buffer PTS it saw.
fn collect_video_pts(path: &Path, budget: Duration) -> Result<Vec<u64>, SubError> {
    let pipeline = gst::Pipeline::new();
    let src = gst::ElementFactory::make("filesrc")
        .property("location", path)
        .build()
        .sub_context(codes::UNSUPPORTED, "filesrc is unavailable")?;
    let parsebin = gst::ElementFactory::make("parsebin")
        .build()
        .sub_context(codes::UNSUPPORTED, "parsebin is unavailable")?;
    pipeline
        .add_many([&src, &parsebin])
        .sub_context(codes::PROBE_FAILED, "could not build the scan pipeline")?;
    src.link(&parsebin)
        .sub_context(codes::PROBE_FAILED, "could not link the scan pipeline")?;

    let timestamps = Arc::new(Mutex::new(Vec::<u64>::new()));
    let collected = Arc::clone(&timestamps);
    let weak_pipeline = pipeline.downgrade();
    parsebin.connect_pad_added(move |_, pad| {
        let Some(pipeline) = weak_pipeline.upgrade() else {
            return;
        };
        // Every pad needs a consumer or the parser stalls, but only the video
        // pads are timed.
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
        let Some(sink_pad) = sink.static_pad("sink") else {
            return;
        };
        if pad.link(&sink_pad).is_err() {
            return;
        }
        let collected = Arc::clone(&collected);
        // Whether this pad carries video is decided on its first buffer, not
        // here: a pad can be added before its caps are negotiated.
        let is_video = OnceLock::new();
        pad.add_probe(gst::PadProbeType::BUFFER, move |pad, probe| {
            if *is_video.get_or_init(|| pad_carries_video(pad))
                && let Some(gst::PadProbeData::Buffer(buffer)) = &probe.data
                && let Some(pts) = buffer.pts()
                && let Ok(mut seen) = collected.lock()
                && seen.len() < MAX_SCANNED_FRAMES
            {
                seen.push(pts.nseconds());
            }
            gst::PadProbeReturn::Ok
        });
    });

    let outcome = run_until_eos(&pipeline, budget);
    let _ = pipeline.set_state(gst::State::Null);
    outcome?;

    let seen = timestamps
        .lock()
        .map_err(|_| SubError::new(codes::PROBE_FAILED, "frame timing scan panicked"))?;
    Ok(seen.clone())
}

/// Whether `pad` carries video, judged from its caps once data flows.
///
/// A parser pad can be added before its caps are negotiated, so the caps are
/// read when the first buffer arrives rather than at `pad-added`; the pad's
/// allowed caps are the fallback for the rare pad that reports none.
fn pad_carries_video(pad: &gst::Pad) -> bool {
    let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
    media_type(Some(&caps)).is_some_and(|name| name.starts_with("video/"))
}

/// Upper bound on collected timestamps: ten hours at 60 fps, enough to classify
/// any real source without an unbounded allocation.
const MAX_SCANNED_FRAMES: usize = 2_160_000;

/// Plays `pipeline` until it reaches end of stream, errors, or runs out of
/// `budget`.
fn run_until_eos(pipeline: &gst::Pipeline, budget: Duration) -> Result<(), SubError> {
    pipeline
        .set_state(gst::State::Playing)
        .sub_context(codes::PROBE_FAILED, "the scan pipeline would not start")?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| SubError::new(codes::PROBE_FAILED, "the scan pipeline has no bus"))?;
    let deadline = Instant::now() + budget;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(SubError::new(
                codes::PROBE_TIMEOUT,
                "frame timing scan ran out of time",
            ));
        }
        let Some(message) = bus.timed_pop(clock_time(left)) else {
            return Err(SubError::new(
                codes::PROBE_TIMEOUT,
                "frame timing scan ran out of time",
            ));
        };
        match message.view() {
            gst::MessageView::Eos(_) => return Ok(()),
            gst::MessageView::Error(err) => {
                return Err(SubError::wrap(
                    codes::PROBE_FAILED,
                    "frame timing scan failed",
                    &err.error(),
                ));
            }
            _ => {}
        }
    }
}

/// Classifies frame spacing from the timestamps the scan collected.
///
/// Timestamps are sorted first: a long-GOP stream is parsed in decode order, so
/// presentation timestamps arrive out of order and only their sorted spacing
/// says anything about frame duration. Duplicate timestamps are dropped, since
/// a zero-length gap is a container quirk rather than a frame duration.
fn classify_timing(timestamps: &mut Vec<u64>) -> FrameTiming {
    timestamps.sort_unstable();
    timestamps.dedup();
    if timestamps.len() < 3 {
        return FrameTiming::Unknown;
    }
    let mut shortest = u64::MAX;
    let mut longest = 0_u64;
    for pair in timestamps.windows(2) {
        let delta = pair[1] - pair[0];
        shortest = shortest.min(delta);
        longest = longest.max(delta);
    }
    if shortest == 0 {
        return FrameTiming::Unknown;
    }
    if u128::from(longest) * TIMING_TOLERANCE_DEN > u128::from(shortest) * TIMING_TOLERANCE_NUM {
        FrameTiming::Variable
    } else {
        FrameTiming::Constant
    }
}

/// A probed file as the Command API reports it (`media.probe`).
///
/// [`MediaInfo`] is not a serde type — it is what the prober returns, not
/// something the project file stores — so the wire shape is written here, once,
/// for every process that serves the method: the editor and
/// `subordinate-cli serve` answer with the same document rather than with two
/// hand-written copies of it. Every time in it is an exact rational.
#[must_use]
pub fn media_info_json(info: &MediaInfo, path: &Path) -> serde_json::Value {
    use serde_json::{Value, json};
    json!({
        "path": path.display().to_string(),
        "container": info.container,
        "container_format": info.container_format,
        "duration": info.duration,
        "seekable": info.seekable,
        "variable_frame_rate": info.is_variable_frame_rate(),
        "video": info.video.iter().map(|video| json!({
            "codec": video.codec,
            "codec_description": video.codec_description,
            "width": video.width,
            "height": video.height,
            "frame_rate": video.frame_rate.map(|rate| json!({
                "numerator": rate.numerator(),
                "denominator": rate.denominator(),
            })),
            "sample_aspect": {
                "numerator": video.sample_aspect.numerator(),
                "denominator": video.sample_aspect.denominator(),
            },
            "rotation_degrees": video.rotation.degrees(),
            "mirrored": video.mirrored,
            "interlaced": video.interlaced,
            "variable_frame_rate": video.timing.is_variable(),
        })).collect::<Vec<Value>>(),
        "audio": info.audio.iter().map(|audio| json!({
            "codec": audio.codec,
            "codec_description": audio.codec_description,
            "channels": audio.channels,
            "sample_rate": audio.sample_rate,
            "language": audio.language,
        })).collect::<Vec<Value>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ColorPrimaries, ColorSpace, FrameTiming, NANOSECONDS, Rotation, TransferFunction,
        classify_timing, color_tags, duration_of,
    };

    #[test]
    fn nanosecond_durations_are_exact() {
        let duration = duration_of(5_000_000_000);
        assert_eq!(duration.value(), 5_000_000_000);
        assert_eq!(duration.rate(), NANOSECONDS);
        assert_eq!(duration.as_seconds_fraction(), (5, 1));
    }

    #[test]
    fn orientation_tags_parse() {
        assert_eq!(Rotation::from_tag("rotate-0"), (Rotation::None, false));
        assert_eq!(Rotation::from_tag("rotate-90"), (Rotation::Cw90, false));
        assert_eq!(Rotation::from_tag("rotate-180"), (Rotation::Cw180, false));
        assert_eq!(Rotation::from_tag("rotate-270"), (Rotation::Cw270, false));
        assert_eq!(Rotation::from_tag("flip-rotate-90"), (Rotation::Cw90, true));
        assert_eq!(Rotation::from_tag("nonsense"), (Rotation::None, false));
        assert_eq!(Rotation::Cw270.degrees(), 270);
    }

    #[test]
    fn constant_spacing_is_not_variable() {
        // 25 fps: 40 ms exactly.
        let mut pts: Vec<u64> = (0..50).map(|i| i * 40_000_000).collect();
        assert_eq!(classify_timing(&mut pts), FrameTiming::Constant);
    }

    #[test]
    fn one_nanosecond_wobble_is_still_constant() {
        // 30000/1001 fps: frame durations alternate by a nanosecond.
        let mut pts: Vec<u64> = (0..50).map(|i: u64| i * 33_366_666 + i / 3).collect();
        assert_eq!(classify_timing(&mut pts), FrameTiming::Constant);
    }

    #[test]
    fn a_rate_change_is_variable() {
        // 3 s at 30 fps then 3 s at 60 fps, the shape of the VFR fixture.
        let mut pts: Vec<u64> = (0..90).map(|i| i * 33_333_333).collect();
        let base = 3_000_000_000_u64;
        pts.extend((0..180).map(|i| base + i * 16_666_666));
        assert_eq!(classify_timing(&mut pts), FrameTiming::Variable);
    }

    #[test]
    fn out_of_order_timestamps_are_sorted_before_judging() {
        // Decode order for a stream with B-frames: presentation order is
        // recovered by sorting, so the stream is still constant.
        let mut pts = vec![
            0_u64,
            120_000_000,
            40_000_000,
            80_000_000,
            240_000_000,
            160_000_000,
            200_000_000,
        ];
        assert_eq!(classify_timing(&mut pts), FrameTiming::Constant);
    }

    #[test]
    fn too_few_timestamps_stay_unknown() {
        let mut pts = vec![0_u64, 40_000_000];
        assert_eq!(classify_timing(&mut pts), FrameTiming::Unknown);
        let mut none: Vec<u64> = Vec::new();
        assert_eq!(classify_timing(&mut none), FrameTiming::Unknown);
    }

    #[test]
    fn colorimetry_strings_map_onto_model_tags() {
        gstreamer::init().expect("GStreamer must initialise");
        let tags = color_tags("bt709");
        assert_eq!(tags.space, ColorSpace::Rec709);
        assert_eq!(tags.transfer, TransferFunction::Bt709);
        assert_eq!(tags.primaries, ColorPrimaries::Bt709);

        let tags = color_tags("bt601");
        assert_eq!(tags.space, ColorSpace::Rec601);
        assert_eq!(tags.primaries, ColorPrimaries::Bt601);

        let tags = color_tags("bt2100-pq");
        assert_eq!(tags.space, ColorSpace::Rec2020);
        assert_eq!(tags.transfer, TransferFunction::Pq);
        assert_eq!(tags.primaries, ColorPrimaries::Bt2020);

        // Unparseable colorimetry falls back to the default tags.
        assert_eq!(color_tags("not-a-colorimetry"), super::ColorTags::default());
    }
}
