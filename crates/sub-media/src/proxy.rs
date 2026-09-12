//! Proxies: intra-only stand-ins for long-GOP sources (docs/PLAN.md §5.2).
//!
//! A long-GOP camera file scrubs badly, because every picture the playhead
//! lands on needs the whole GOP in front of it decoded first. A proxy is the
//! same pictures re-encoded intra-only — `DNxHR` LB or MJPEG — at half or
//! quarter resolution, so a seek costs one frame. Export always uses the
//! original; the proxy exists only to make editing feel immediate.
//!
//! Everything about a proxy is derived from the source's [`ContentHash`] and
//! its [`ProxyOptions`], so the file name is the same on every machine and
//! across runs, and a proxy already sitting in the sidecar directory is
//! reused rather than made again. Generation runs on the
//! [`JobService`](sub_core::JobService) like every other background job: it
//! reports progress in frames, and it stops promptly when it is cancelled.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use std::path::Path;
//! use sub_core::{JobService, Priority};
//! use sub_media::{Proxy, ProxyOptions, spawn_proxy_job};
//!
//! let jobs = JobService::with_default_workers();
//! let job = spawn_proxy_job(
//!     &jobs,
//!     Path::new("/media/take-1.mov"),
//!     Path::new("/projects/cut.sub.d"),
//!     ProxyOptions::default(),
//!     Priority::Background,
//! );
//! let proxy: Proxy = job.wait()?;
//! println!("{} frames", proxy.frame_count());
//! # Ok(())
//! # }
//! ```
//!
//! # Variable frame rate
//!
//! Nothing here assumes a constant frame duration. The source's
//! [`PtsIndex`] is built (or read from the sidecar cache) before the
//! transcode, and the timestamps the decoder produces are carried through the
//! pipeline untouched — no `videorate`, no rate conversion. Afterwards the
//! proxy's own index is built and compared with the source's frame for frame,
//! and a proxy whose frames do not line up is rejected rather than written.
//! That is what lets a caller treat proxy frame *n* as original frame *n* on a
//! VFR source (TASK-16).
//!
//! # Auto-generation
//!
//! [`ProxyPolicy`] decides which imports are worth proxying: a configurable
//! resolution threshold, and — unless the caller asks otherwise — only for
//! codecs that actually carry long GOPs. It also picks the codec, preferring
//! `DNxHR` LB and falling back to MJPEG on an installation whose GStreamer
//! cannot encode or mux `DNxHR` at the proxy size.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};
use sub_core::{
    CancelToken, JobContext, JobHandle, JobService, Priority, ResultExt, SubError, SubResult,
};
use sub_model::ContentHash;
use sub_time::RationalTime;

use crate::codes;
use crate::decode::{prefer_hardware_decoders, readable_file_uri};
use crate::index::PtsIndex;
use crate::probe::{MediaInfo, NANOSECONDS, probe};

/// Schema version of the proxy manifest. A manifest written by an older
/// version is regenerated rather than trusted.
pub const PROXY_MANIFEST_VERSION: u32 = 1;

/// The name every proxy job is submitted under.
pub const PROXY_JOB_KIND: &str = "proxy";

/// Source width at or below which [`ProxyPolicy`] leaves a file alone.
pub const DEFAULT_PROXY_MIN_WIDTH: u32 = 1920;

/// Source height at or below which [`ProxyPolicy`] leaves a file alone.
pub const DEFAULT_PROXY_MIN_HEIGHT: u32 = 1080;

/// Target bit rate of the `DNxHR` LB encoder, in bits per second. LB is the
/// lowest-bandwidth `DNxHR` profile; this is what its rate control is aimed at
/// for a half-resolution HD-sized proxy.
const DNXHR_LB_BITRATE: i32 = 36_000_000;

/// How long the transcode may make no progress at all before it is called a
/// stall. It bounds a wedged pipeline, not the transcode, which is expected
/// to take minutes on a long source.
const STALL_TIMEOUT: Duration = Duration::from_mins(1);

/// How far a proxy frame may sit from its source frame, measured from each
/// file's own first frame, and still count as the same frame.
///
/// The pictures are never retimed, but two things move a timestamp without
/// moving a picture: a container that starts its timeline at something other
/// than zero (an MP4 whose first presentation timestamp sits after its first
/// decode timestamp), and the rounding of a timescale on the way out. Both are
/// answered by comparing offsets from each file's first frame with this
/// tolerance, which is orders of magnitude below any frame duration and so
/// cannot hide an off-by-one.
pub const MAX_PROXY_PTS_DRIFT_NS: i64 = 1_000_000;

/// The intra-only codec a proxy is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyCodec {
    /// Avid `DNxHR` LB: the editorial standard for proxies, and the first
    /// choice when this installation can encode and mux it.
    DnxhrLb,
    /// Motion JPEG: every GStreamer installation has `jpegenc`, so this is
    /// the fallback that always works.
    Mjpeg,
}

/// Every proxy codec, in the order [`ProxyPolicy`] prefers them.
pub const PROXY_CODECS: [ProxyCodec; 2] = [ProxyCodec::DnxhrLb, ProxyCodec::Mjpeg];

impl ProxyCodec {
    /// The stable identifier used in file names, JSON and the command API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DnxhrLb => "dnxhr_lb",
            Self::Mjpeg => "mjpeg",
        }
    }

    /// A human-readable name for the UI.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::DnxhrLb => "DNxHR LB",
            Self::Mjpeg => "MJPEG",
        }
    }

    /// Parses the identifier [`ProxyCodec::as_str`] writes.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        PROXY_CODECS.into_iter().find(|c| c.as_str() == name)
    }

    /// The GStreamer encoder element that writes it.
    #[must_use]
    pub fn encoder_element(self) -> &'static str {
        match self {
            Self::DnxhrLb => "avenc_dnxhd",
            Self::Mjpeg => "jpegenc",
        }
    }

    /// The GStreamer muxer the coded stream is wrapped in.
    #[must_use]
    pub fn muxer_element(self) -> &'static str {
        "qtmux"
    }

    /// The extension of the written file, without a dot.
    #[must_use]
    pub fn file_extension(self) -> &'static str {
        "mov"
    }

    /// The raw pixel format handed to the encoder: 4:2:2 for `DNxHR`, which has
    /// no 4:2:0 form, and 4:2:0 for MJPEG, which is what the decoders upstream
    /// already produce.
    fn raw_format(self) -> &'static str {
        match self {
            Self::DnxhrLb => "Y42B",
            Self::Mjpeg => "I420",
        }
    }

    /// The media type of the coded stream, as the muxer sees it and as a
    /// probe of the written proxy reports it.
    #[must_use]
    pub fn coded_media_type(self) -> &'static str {
        match self {
            Self::DnxhrLb => "video/x-dnxhd",
            Self::Mjpeg => "image/jpeg",
        }
    }

    /// Whether this installation can both encode and mux the codec at a given
    /// proxy size.
    ///
    /// `DNxHR` is picky: a build of `gst-libav` may only accept the handful of
    /// `DNxHD` frame sizes, and a build with no muxer that takes `video/x-dnxhd`
    /// cannot write the file at all. Asking the registry is how the policy
    /// avoids choosing a codec that would fail halfway through a transcode.
    #[must_use]
    pub fn is_available_at(self, width: u32, height: u32) -> bool {
        if gst::init().is_err() {
            return false;
        }
        let raw = size_caps(
            gst::Caps::builder("video/x-raw").field("format", self.raw_format()),
            width,
            height,
        );
        let coded = size_caps(gst::Caps::builder(self.coded_media_type()), width, height);
        factory_accepts(self.encoder_element(), &raw)
            && factory_accepts(self.muxer_element(), &coded)
    }

    /// The first codec of `PROXY_CODECS` this installation can write at the
    /// given proxy size, or [`ProxyCodec::Mjpeg`] when it can write none —
    /// generation then fails with a clear `media.unsupported` rather than
    /// silently doing nothing.
    #[must_use]
    pub fn preferred_at(width: u32, height: u32) -> Self {
        PROXY_CODECS
            .into_iter()
            .find(|codec| codec.is_available_at(width, height))
            .unwrap_or(Self::Mjpeg)
    }
}

impl std::fmt::Display for ProxyCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How far down a proxy is scaled from its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyScale {
    /// Half the source's width and height: a quarter of the pixels.
    Half,
    /// A quarter of the source's width and height: a sixteenth of the pixels.
    Quarter,
}

impl ProxyScale {
    /// The number each source dimension is divided by.
    #[must_use]
    pub fn divisor(self) -> u32 {
        match self {
            Self::Half => 2,
            Self::Quarter => 4,
        }
    }

    /// The stable identifier used in file names and JSON.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Half => "half",
            Self::Quarter => "quarter",
        }
    }

    /// Parses the identifier [`ProxyScale::as_str`] writes.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Half, Self::Quarter]
            .into_iter()
            .find(|scale| scale.as_str() == name)
    }
}

impl std::fmt::Display for ProxyScale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a proxy is generated.
///
/// The options are part of a proxy's identity: two different option sets
/// produce two different files, so changing them never leaves a caller
/// previewing something made for other settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyOptions {
    /// The intra-only codec the proxy is written in.
    pub codec: ProxyCodec,
    /// How far the pictures are scaled down.
    pub scale: ProxyScale,
    /// Encoder quality, 1..=100. Only MJPEG reads it; `DNxHR` LB is a fixed
    /// profile with its own rate control.
    pub quality: u8,
}

impl Default for ProxyOptions {
    fn default() -> Self {
        Self {
            codec: ProxyCodec::Mjpeg,
            scale: ProxyScale::Half,
            quality: 75,
        }
    }
}

impl ProxyOptions {
    /// Rejects options no proxy could be made from.
    ///
    /// # Errors
    ///
    /// Returns `core.invalid_argument` when the quality is outside 1..=100.
    pub fn validate(self) -> SubResult<()> {
        if self.quality == 0 || self.quality > 100 {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "proxy quality must be between 1 and 100",
            )
            .with_detail("quality", self.quality));
        }
        Ok(())
    }

    /// The part of a file name that distinguishes one option set from another.
    #[must_use]
    pub fn fingerprint(self) -> String {
        format!(
            "{}-{}-q{}",
            self.codec.as_str(),
            self.scale.as_str(),
            self.quality
        )
    }
}

/// When a source is worth proxying, and how.
///
/// The threshold is the whole point: proxying an HD file costs more than it
/// saves, and proxying a 6K long-GOP file is the difference between scrubbing
/// and waiting. Both dimensions are configurable, and so is whether
/// intra-only sources — which already scrub fine — are proxied too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyPolicy {
    /// Whether auto-generation happens at all.
    pub enabled: bool,
    /// A source wider than this is proxied.
    pub min_width: u32,
    /// A source taller than this is proxied.
    pub min_height: u32,
    /// Whether sources that are already intra-only are proxied too. Off by
    /// default: an intra-only source scrubs at full resolution already, and a
    /// proxy of it would only cost disk.
    pub include_intra_sources: bool,
    /// How far down generated proxies are scaled.
    pub scale: ProxyScale,
    /// Encoder quality for generated proxies.
    pub quality: u8,
}

impl Default for ProxyPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            min_width: DEFAULT_PROXY_MIN_WIDTH,
            min_height: DEFAULT_PROXY_MIN_HEIGHT,
            include_intra_sources: false,
            scale: ProxyScale::Half,
            quality: ProxyOptions::default().quality,
        }
    }
}

impl ProxyPolicy {
    /// Whether importing `info` should queue a proxy job.
    #[must_use]
    pub fn should_generate(&self, info: &MediaInfo) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(video) = info.video.first() else {
            return false;
        };
        let big_enough = video.width > self.min_width || video.height > self.min_height;
        big_enough && (self.include_intra_sources || is_long_gop(&video.codec))
    }

    /// The options a proxy of `info` would be generated with, or `None` when
    /// this source is below the threshold.
    ///
    /// The codec is chosen for the size the proxy will actually be, so an
    /// installation that cannot encode `DNxHR` at that size gets MJPEG instead
    /// of a job that fails.
    #[must_use]
    pub fn options_for(&self, info: &MediaInfo) -> Option<ProxyOptions> {
        if !self.should_generate(info) {
            return None;
        }
        let video = info.video.first()?;
        let (width, height) = proxy_size(video.width, video.height, self.scale);
        Some(ProxyOptions {
            codec: ProxyCodec::preferred_at(width, height),
            scale: self.scale,
            quality: self.quality,
        })
    }
}

/// Whether a coded media type carries long GOPs, and so scrubs badly.
///
/// The list is of the intra-only families an editor actually meets; anything
/// unknown is assumed to be long-GOP, because that is the case where a proxy
/// helps and a wrong guess only costs disk.
#[must_use]
pub fn is_long_gop(codec: &str) -> bool {
    const INTRA_ONLY: [&str; 8] = [
        "image/jpeg",
        "video/x-dnxhd",
        "video/x-prores",
        "video/x-dv",
        "video/x-huffyuv",
        "video/x-ffv",
        "video/x-raw",
        "video/x-jpeg",
    ];
    !INTRA_ONLY
        .iter()
        .any(|intra| codec.eq_ignore_ascii_case(intra))
}

/// The size a proxy of a source is written at: each dimension divided by the
/// scale, rounded down to an even number and never below two.
///
/// Even dimensions are not cosmetic: a 4:2:0 or 4:2:2 encoder cannot express
/// an odd width, and the scaler would have to pad.
#[must_use]
pub fn proxy_size(source_width: u32, source_height: u32, scale: ProxyScale) -> (u32, u32) {
    let divisor = scale.divisor();
    let shrink = |value: u32| ((value / divisor) & !1).max(2);
    (shrink(source_width), shrink(source_height))
}

/// The on-disk manifest of a proxy, and what [`Proxy`] round-trips.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManifestFile {
    version: u32,
    source_hash: String,
    options: ProxyOptions,
    file: String,
    width: u32,
    height: u32,
    frame_count: usize,
}

/// A generated proxy: where the file is, what shape it has, and how many
/// frames it holds.
///
/// The frame count is the source's: a proxy is only written once its frames
/// have been checked to line up with the original's one for one, so frame *n*
/// of a proxy is frame *n* of the source even on a variable-frame-rate file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    cache_dir: PathBuf,
    source_hash: ContentHash,
    options: ProxyOptions,
    file: String,
    width: u32,
    height: u32,
    frame_count: usize,
}

impl Proxy {
    /// The sidecar directory holding the proxy.
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// The hash of the source the proxy was made from.
    #[must_use]
    pub fn source_hash(&self) -> ContentHash {
        self.source_hash
    }

    /// The options the proxy was made with.
    #[must_use]
    pub fn options(&self) -> ProxyOptions {
        self.options
    }

    /// Where the proxy file lives.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.cache_dir.join(&self.file)
    }

    /// Pixel width of the proxy.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Pixel height of the proxy.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// How many pictures the proxy holds, which is how many the source holds.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// File name of the proxy of a source with `hash`.
    #[must_use]
    pub fn file_name(hash: ContentHash, options: ProxyOptions) -> String {
        format!(
            "{hash}.{}.proxy.{}",
            options.fingerprint(),
            options.codec.file_extension()
        )
    }

    /// File name of the manifest beside it.
    #[must_use]
    pub fn manifest_file_name(hash: ContentHash, options: ProxyOptions) -> String {
        format!("{hash}.{}.proxy.json", options.fingerprint())
    }

    /// Reads a proxy back from the sidecar directory.
    ///
    /// Returns `None` when there is no manifest, when it was written by
    /// another schema version, for other bytes or for other options, or when
    /// the file it names is missing — every one of which is a reason to
    /// generate the proxy again rather than to fail.
    #[must_use]
    pub fn load(cache_dir: &Path, hash: ContentHash, options: ProxyOptions) -> Option<Self> {
        let manifest_path = cache_dir.join(Self::manifest_file_name(hash, options));
        let text = std::fs::read_to_string(manifest_path).ok()?;
        let manifest: ManifestFile = serde_json::from_str(&text).ok()?;
        if manifest.version != PROXY_MANIFEST_VERSION
            || manifest.options != options
            || manifest.source_hash != hash.to_string()
            || manifest.file != Self::file_name(hash, options)
        {
            return None;
        }
        let proxy = Self {
            cache_dir: cache_dir.to_path_buf(),
            source_hash: hash,
            options,
            file: manifest.file,
            width: manifest.width,
            height: manifest.height,
            frame_count: manifest.frame_count,
        };
        is_written(&proxy.path()).then_some(proxy)
    }

    /// Generates the proxy for `path` into `cache_dir`, reusing one that is
    /// already there.
    ///
    /// # Errors
    ///
    /// Returns `media.proxy_failed` when the source cannot be transcoded or
    /// the result does not line up with the original, `media.unsupported`
    /// when this installation has no encoder or muxer for the options, and
    /// `core.invalid_argument` for options no proxy could be made from.
    pub fn generate(path: &Path, cache_dir: &Path, options: ProxyOptions) -> SubResult<Self> {
        Self::generate_with(
            path,
            cache_dir,
            options,
            &CancelToken::new(),
            &mut |_, _| {},
        )
    }

    /// Generates the proxy, reporting progress and stopping when `cancel` is
    /// set.
    ///
    /// `progress` is called with `(finished, total)` frames as the transcode
    /// advances; the totals come from the source's PTS index, so they are
    /// exact on a variable-frame-rate file too.
    ///
    /// # Errors
    ///
    /// As [`Proxy::generate`], plus `core.cancelled` when the token is set
    /// before the proxy is complete.
    pub fn generate_with(
        path: &Path,
        cache_dir: &Path,
        options: ProxyOptions,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(u64, u64),
    ) -> SubResult<Self> {
        options.validate()?;
        let hash = hash_source(path)?;

        // The cheapest possible path: a finished proxy needs no probe, no
        // index and no transcode.
        if let Some(proxy) = Self::load(cache_dir, hash, options) {
            tracing::debug!(path = %path.display(), "proxy already generated");
            let total = u64::try_from(proxy.frame_count).unwrap_or(u64::MAX);
            progress(total, total);
            return Ok(proxy);
        }
        check_cancelled(cancel)?;

        let info = probe(path).map_err(|err| failed("media file cannot be probed", path, &err))?;
        let video = info.video.first().ok_or_else(|| {
            SubError::new(codes::NO_VIDEO_STREAM, "media file carries no video stream")
                .with_detail("path", path.display().to_string())
        })?;
        let (width, height) = proxy_size(video.width, video.height, options.scale);
        if !options.codec.is_available_at(width, height) {
            return Err(SubError::new(
                codes::UNSUPPORTED,
                "this installation cannot write a proxy in the requested codec at this size",
            )
            .with_detail("codec", options.codec.as_str())
            .with_detail("width", width)
            .with_detail("height", height));
        }

        // The index is what makes a VFR source safe: it gives the exact frame
        // count to report progress against, and the timestamps the finished
        // proxy is checked against.
        let index = PtsIndex::load_or_build(path, cache_dir, cancel)
            .map_err(|err| failed("source PTS index cannot be built", path, &err))?;
        let total = u64::try_from(index.len()).unwrap_or(u64::MAX);
        progress(0, total);

        let file = Self::file_name(hash, options);
        let target = cache_dir.join(&file);
        std::fs::create_dir_all(cache_dir).map_err(|err| {
            SubError::wrap(
                codes::PROXY_FAILED,
                "sidecar directory cannot be created",
                &err,
            )
            .with_detail("path", cache_dir.display().to_string())
        })?;
        // A half-written proxy must never be mistaken for a finished one, so
        // the transcode writes beside the target and renames at the end.
        let partial = target.with_extension("part");
        let transcoded = transcode(
            &TranscodeRequest {
                source: path,
                target: &partial,
                options,
                width,
                height,
            },
            &index,
            cancel,
            progress,
        );
        if let Err(err) = transcoded {
            let _ = std::fs::remove_file(&partial);
            return Err(err);
        }

        let mapped = check_one_to_one(&index, &partial, cancel);
        if let Err(err) = mapped {
            let _ = std::fs::remove_file(&partial);
            return Err(err);
        }

        std::fs::rename(&partial, &target).map_err(|err| {
            SubError::wrap(
                codes::PROXY_FAILED,
                "proxy cannot be moved into place",
                &err,
            )
            .with_detail("path", target.display().to_string())
        })?;
        let proxy = Self {
            cache_dir: cache_dir.to_path_buf(),
            source_hash: hash,
            options,
            file,
            width,
            height,
            frame_count: index.len(),
        };
        proxy.write_manifest()?;
        progress(total, total);
        tracing::info!(
            path = %path.display(),
            proxy = %proxy.path().display(),
            codec = options.codec.as_str(),
            frames = proxy.frame_count,
            "proxy generated"
        );
        Ok(proxy)
    }

    /// Writes the manifest beside the proxy, atomically.
    fn write_manifest(&self) -> SubResult<()> {
        let path = self
            .cache_dir
            .join(Self::manifest_file_name(self.source_hash, self.options));
        let manifest = ManifestFile {
            version: PROXY_MANIFEST_VERSION,
            source_hash: self.source_hash.to_string(),
            options: self.options,
            file: self.file.clone(),
            width: self.width,
            height: self.height,
            frame_count: self.frame_count,
        };
        let text = serde_json::to_string(&manifest)
            .sub_context(codes::PROXY_FAILED, "proxy manifest cannot be serialised")?;
        let temporary = path.with_extension("tmp");
        let io = |err: &std::io::Error| {
            SubError::wrap(codes::PROXY_FAILED, "proxy manifest cannot be written", err)
                .with_detail("path", path.display().to_string())
        };
        std::fs::write(&temporary, text.as_bytes()).map_err(|err| io(&err))?;
        std::fs::rename(&temporary, &path).map_err(|err| io(&err))
    }
}

/// A proxy job running on a [`JobService`].
#[derive(Debug, Clone)]
pub struct ProxyJob {
    handle: JobHandle,
    proxy: Arc<Mutex<Option<Proxy>>>,
}

impl ProxyJob {
    /// The underlying job handle: state, priority, cancellation.
    #[must_use]
    pub fn handle(&self) -> &JobHandle {
        &self.handle
    }

    /// Asks the job to stop; it does so between bus messages.
    pub fn cancel(&self) {
        self.handle.cancel();
    }

    /// Blocks until the job finishes and returns the proxy.
    ///
    /// # Errors
    ///
    /// Returns the job's error, or `core.cancelled` when it was cancelled.
    pub fn wait(&self) -> SubResult<Proxy> {
        self.handle.wait().into_result()?;
        let proxy = self
            .proxy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        proxy.ok_or_else(|| {
            SubError::new(
                sub_core::codes::INTERNAL,
                "a completed proxy job left no proxy",
            )
        })
    }
}

/// Queues proxy generation for `path` on `jobs`.
///
/// Returns at once; the proxy is transcoded on a worker thread and progress
/// reaches [`JobService::subscribe`] subscribers as
/// [`JobEvent::Progress`](sub_core::JobEvent::Progress) with the finished and
/// total frame counts. Proxies are background work behind thumbnails and
/// indexing, so [`Priority::Background`] is the usual choice.
pub fn spawn_proxy_job(
    jobs: &JobService,
    path: &Path,
    cache_dir: &Path,
    options: ProxyOptions,
    priority: Priority,
) -> ProxyJob {
    let path = path.to_path_buf();
    let cache_dir = cache_dir.to_path_buf();
    let proxy = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&proxy);
    let handle = jobs.submit(PROXY_JOB_KIND, priority, move |ctx: &JobContext| {
        let cancel = ctx.cancel_token();
        let generated =
            Proxy::generate_with(&path, &cache_dir, options, &cancel, &mut |done, total| {
                ctx.progress(done, total);
            })?;
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(generated);
        Ok(())
    });
    ProxyJob { handle, proxy }
}

/// Everything the transcode pipeline needs to know about one proxy.
struct TranscodeRequest<'a> {
    source: &'a Path,
    target: &'a Path,
    options: ProxyOptions,
    width: u32,
    height: u32,
}

/// Transcodes the source into an intra-only file at the proxy size.
///
/// The pipeline is `uridecodebin` — hardware decoders ranked first, exactly as
/// the preview decoder ranks them — into `videoconvert ! videoscale !
/// capsfilter ! encoder ! muxer ! filesink`. There is deliberately no
/// `videorate`: buffer timestamps pass through untouched, which is what keeps
/// a variable-frame-rate source's frames where they were.
fn transcode(
    request: &TranscodeRequest<'_>,
    index: &PtsIndex,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(u64, u64),
) -> SubResult<()> {
    gst::init().map_err(|e| SubError::wrap(codes::INIT_FAILED, "GStreamer failed to start", &e))?;
    let uri = readable_file_uri(request.source)?;
    prefer_hardware_decoders();

    let pipeline = gst::Pipeline::new();
    let source = gst::ElementFactory::make("uridecodebin")
        .property("uri", &uri)
        .property("expose-all-streams", false)
        .property("caps", decoded_video_caps())
        .build()
        .sub_context(codes::UNSUPPORTED, "uridecodebin is unavailable")?;
    let queue = make("queue")?;
    let convert = make("videoconvert")?;
    let scale = make("videoscale")?;
    let filter = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            size_caps(
                gst::Caps::builder("video/x-raw")
                    .field("format", request.options.codec.raw_format()),
                request.width,
                request.height,
            ),
        )
        .build()
        .sub_context(codes::UNSUPPORTED, "capsfilter is unavailable")?;
    let encoder = build_encoder(request.options)?;
    let muxer = make(request.options.codec.muxer_element())?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", request.target)
        .build()
        .sub_context(codes::UNSUPPORTED, "filesink is unavailable")?;

    let branch = [&queue, &convert, &scale, &filter, &encoder, &muxer, &sink];
    pipeline
        .add(&source)
        .sub_context(codes::PROXY_FAILED, "could not build the proxy pipeline")?;
    pipeline
        .add_many(branch)
        .sub_context(codes::PROXY_FAILED, "could not build the proxy pipeline")?;
    gst::Element::link_many(branch)
        .sub_context(codes::PROXY_FAILED, "could not link the proxy pipeline")?;

    // A file with several video streams exposes several pads; the first is
    // transcoded and any later one is dropped rather than left dangling.
    let linked = Arc::new(AtomicBool::new(false));
    let taken = Arc::clone(&linked);
    let queue_sink = queue
        .static_pad("sink")
        .ok_or_else(|| SubError::new(codes::PROXY_FAILED, "the proxy queue exposes no sink pad"))?;
    source.connect_pad_added(move |_, pad| {
        if taken.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Err(err) = pad.link(&queue_sink) {
            tracing::warn!(%err, "decoded pad could not be linked to the proxy encoder");
        }
    });

    let result = run_to_eos(&pipeline, request, index, cancel, progress);
    if let Err(err) = pipeline.set_state(gst::State::Null) {
        tracing::warn!(%err, "proxy pipeline did not shut down cleanly");
    }
    result
}

/// Starts the pipeline and pumps its bus until it reaches the end of the
/// stream, reporting progress in source frames along the way.
fn run_to_eos(
    pipeline: &gst::Pipeline,
    request: &TranscodeRequest<'_>,
    index: &PtsIndex,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(u64, u64),
) -> SubResult<()> {
    let path = request.source;
    pipeline.set_state(gst::State::Playing).map_err(|e| {
        SubError::wrap(
            codes::PROXY_FAILED,
            "the proxy pipeline would not start",
            &e,
        )
        .with_detail("path", path.display().to_string())
    })?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| SubError::new(codes::PROXY_FAILED, "the proxy pipeline has no bus"))?;

    let total = u64::try_from(index.len()).unwrap_or(u64::MAX);
    let mut reported = 0_u64;
    let mut last_change = Instant::now();
    loop {
        if cancel.is_cancelled() {
            return Err(CancelToken::cancelled_error("the proxy job"));
        }
        if let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) {
            match message.view() {
                gst::MessageView::Eos(..) => break,
                gst::MessageView::Error(err) => {
                    return Err(SubError::wrap(
                        codes::PROXY_FAILED,
                        "the proxy pipeline failed",
                        &err.error(),
                    )
                    .with_detail("path", path.display().to_string())
                    .with_detail("codec", request.options.codec.as_str()));
                }
                _ => {}
            }
        }
        let done = transcoded_frames(pipeline, index).min(total);
        if done != reported {
            reported = done;
            last_change = Instant::now();
            progress(done, total);
        } else if last_change.elapsed() > STALL_TIMEOUT {
            return Err(SubError::new(
                codes::DECODE_TIMEOUT,
                "the proxy pipeline made no progress within the stall budget",
            )
            .with_detail("path", path.display().to_string())
            .with_detail("frames", done));
        }
    }
    Ok(())
}

/// How many source frames the pipeline has passed, from its position and the
/// source's PTS index. On a VFR source this is exact where dividing the
/// position by a nominal frame duration would not be.
fn transcoded_frames(pipeline: &gst::Pipeline, index: &PtsIndex) -> u64 {
    let Some(position) = pipeline.query_position::<gst::ClockTime>() else {
        return 0;
    };
    let time = RationalTime::new(
        i64::try_from(position.nseconds()).unwrap_or(i64::MAX),
        NANOSECONDS,
    );
    index
        .frame_at(time)
        .map_or(0, |frame| u64::try_from(frame).unwrap_or(u64::MAX) + 1)
}

/// Builds the encoder element with the settings its codec needs.
fn build_encoder(options: ProxyOptions) -> SubResult<gst::Element> {
    let name = options.codec.encoder_element();
    let builder = gst::ElementFactory::make(name);
    let builder = match options.codec {
        ProxyCodec::Mjpeg => builder.property("quality", i32::from(options.quality)),
        ProxyCodec::DnxhrLb => builder.property("bitrate", DNXHR_LB_BITRATE),
    };
    builder.build().sub_context_with(codes::UNSUPPORTED, || {
        format!("the proxy encoder {name} is unavailable")
    })
}

/// Checks that the generated proxy's frames line up with the source's, one for
/// one, and returns an error naming the first frame that does not.
///
/// Timestamps are compared as offsets from each file's own first frame: what
/// has to hold is that the *n*-th proxy picture shows the same instant of the
/// take as the *n*-th source picture, and a container is free to start its
/// timeline wherever it likes. The spacing between the frames is what a
/// variable-frame-rate source puts at risk, and the spacing is exactly what
/// this compares.
///
/// This is what makes a proxy of a VFR source trustworthy: the transcode never
/// retimes anything, and this proves it for the file that was actually written
/// rather than assuming it.
fn check_one_to_one(source: &PtsIndex, proxy_path: &Path, cancel: &CancelToken) -> SubResult<()> {
    let proxy = PtsIndex::build_cancellable(proxy_path, cancel)
        .map_err(|err| failed("generated proxy cannot be indexed", proxy_path, &err))?;
    if proxy.len() != source.len() {
        return Err(SubError::new(
            codes::PROXY_FAILED,
            "the proxy holds a different number of frames than its source",
        )
        .with_detail("source_frames", source.len())
        .with_detail("proxy_frames", proxy.len()));
    }
    let source_origin = source.entries().first().map_or(0, |entry| entry.pts_ns);
    let proxy_origin = proxy.entries().first().map_or(0, |entry| entry.pts_ns);
    for (frame, (want, got)) in source.entries().iter().zip(proxy.entries()).enumerate() {
        let wanted = want.pts_ns.saturating_sub(source_origin);
        let written = got.pts_ns.saturating_sub(proxy_origin);
        if wanted.saturating_sub(written).abs() > MAX_PROXY_PTS_DRIFT_NS {
            return Err(SubError::new(
                codes::PROXY_FAILED,
                "a proxy frame does not sit at its source frame's timestamp",
            )
            .with_detail("frame", frame)
            .with_detail("source_offset_ns", wanted)
            .with_detail("proxy_offset_ns", written));
        }
    }
    Ok(())
}

/// Makes an element, reporting a missing one as `media.unsupported`.
fn make(name: &str) -> SubResult<gst::Element> {
    gst::ElementFactory::make(name)
        .build()
        .sub_context_with(codes::UNSUPPORTED, || format!("{name} is unavailable"))
}

/// The caps `uridecodebin` treats as decoded: raw video, including the memory
/// features a hardware decoder attaches to its own surfaces.
fn decoded_video_caps() -> gst::Caps {
    let mut caps = gst::Caps::new_empty();
    if let Some(caps) = caps.get_mut() {
        caps.append_structure_full(
            gst::Structure::new_empty("video/x-raw"),
            Some(gst::CapsFeatures::new_any()),
        );
    }
    caps
}

/// Finishes a caps builder with a pixel size.
fn size_caps(
    builder: gst::caps::Builder<gst::caps::NoFeature>,
    width: u32,
    height: u32,
) -> gst::Caps {
    builder
        .field("width", i32::try_from(width).unwrap_or(i32::MAX))
        .field("height", i32::try_from(height).unwrap_or(i32::MAX))
        .build()
}

/// Whether the named element factory exists and would accept `caps`.
fn factory_accepts(name: &str, caps: &gst::Caps) -> bool {
    gst::ElementFactory::find(name).is_some_and(|factory| factory.can_sink_any_caps(caps))
}

/// `Err(core.cancelled)` when the token is set.
fn check_cancelled(cancel: &CancelToken) -> SubResult<()> {
    if cancel.is_cancelled() {
        return Err(CancelToken::cancelled_error("the proxy job"));
    }
    Ok(())
}

/// Wraps a lower-level failure as this module's code, keeping the original
/// code as a detail so a caller can still tell a missing file from a broken
/// codec.
fn failed(message: &str, path: &Path, cause: &SubError) -> SubError {
    SubError::new(codes::PROXY_FAILED, message.to_owned())
        .with_detail("path", path.display().to_string())
        .with_detail("cause_code", cause.code.as_str())
        .with_cause(cause)
}

/// Hashes the source bytes, reporting an unreadable file in this crate's
/// vocabulary rather than the model's.
fn hash_source(path: &Path) -> SubResult<ContentHash> {
    ContentHash::of_file(path).map_err(|err| {
        SubError::new(
            codes::FILE_UNREADABLE,
            "media file cannot be read for a proxy",
        )
        .with_detail("path", path.display().to_string())
        .with_cause(&err)
    })
}

/// Whether a file is there and holds something. A zero-length file is a
/// leftover, never a proxy.
fn is_written(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

/// The folder a project's proxies are written into, relative to the project
/// file.
pub const PROXY_DIR: &str = "proxies";

/// Transcodes a proxy for one of `project`'s media items and returns its
/// project-relative path.
///
/// This is what `media.make_proxy` does in whichever process is serving the
/// Command API — the editor or `subordinate-cli serve` — so the file lands in
/// the same `proxies/` folder and the path recorded on the media item stays
/// project-relative either way (docs/PLAN.md §5.6).
///
/// Slow: it decodes and re-encodes the whole source, so a caller on a UI
/// thread runs it on a worker.
///
/// # Errors
///
/// `core.not_found` when the project references no such item,
/// `core.invalid_argument` for a source with no video stream, and whatever the
/// prober and the proxy pipeline return.
pub fn proxy_for_media(
    project: &sub_model::Project,
    project_dir: &Path,
    media: sub_model::MediaId,
) -> SubResult<String> {
    let source = project.absolute_path(project_dir, media).ok_or_else(|| {
        SubError::new(
            sub_core::codes::NOT_FOUND,
            "no such media item in the project",
        )
        .with_detail("media", media.to_string())
    })?;
    let info = probe(&source)?;
    let video = info.video.first().ok_or_else(|| {
        SubError::new(
            sub_core::codes::INVALID_ARGUMENT,
            "a proxy needs a source with a video stream",
        )
        .with_detail("media", media.to_string())
    })?;
    let defaults = ProxyOptions::default();
    let (width, height) = proxy_size(video.width, video.height, defaults.scale);
    let options = ProxyOptions {
        codec: ProxyCodec::preferred_at(width, height),
        ..defaults
    };
    let proxy = Proxy::generate(&source, &project_dir.join(PROXY_DIR), options)?;
    let file = proxy.path();
    let relative = file.strip_prefix(project_dir).unwrap_or(&file);
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use crate::probe::{FrameTiming, Rotation, VideoStreamInfo};
    use sub_model::sequence::ColorTags;
    use sub_time::Rational;

    use super::*;

    /// A video stream description with just the fields the policy reads.
    fn video(codec: &str, width: u32, height: u32) -> VideoStreamInfo {
        VideoStreamInfo {
            codec: codec.to_owned(),
            codec_description: codec.to_owned(),
            width,
            height,
            frame_rate: Rational::new(25, 1),
            sample_aspect: Rational::ONE,
            rotation: Rotation::None,
            mirrored: false,
            color: ColorTags::default(),
            interlaced: false,
            timing: FrameTiming::Constant,
        }
    }

    /// A probed file carrying one video stream.
    fn info(codec: &str, width: u32, height: u32) -> MediaInfo {
        MediaInfo {
            container: "video/quicktime".to_owned(),
            container_format: None,
            duration: Some(RationalTime::new(1_000_000_000, NANOSECONDS)),
            seekable: true,
            video: vec![video(codec, width, height)],
            audio: Vec::new(),
        }
    }

    #[test]
    fn proxy_sizes_halve_or_quarter_and_stay_even() {
        assert_eq!(proxy_size(3840, 2160, ProxyScale::Half), (1920, 1080));
        assert_eq!(proxy_size(3840, 2160, ProxyScale::Quarter), (960, 540));
        assert_eq!(proxy_size(1920, 1080, ProxyScale::Quarter), (480, 270));
        // Odd halves round down to an even number the encoders can express.
        assert_eq!(proxy_size(1919, 1079, ProxyScale::Half), (958, 538));
        // Nothing ever collapses to zero.
        assert_eq!(proxy_size(1, 1, ProxyScale::Quarter), (2, 2));
        assert_eq!(proxy_size(0, 0, ProxyScale::Half), (2, 2));
    }

    #[test]
    fn options_validate_their_quality_and_fingerprint_themselves() {
        assert!(ProxyOptions::default().validate().is_ok());
        for quality in [0, 101] {
            let err = ProxyOptions {
                quality,
                ..ProxyOptions::default()
            }
            .validate()
            .unwrap_err();
            assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
        }
        assert_eq!(
            ProxyOptions {
                codec: ProxyCodec::DnxhrLb,
                scale: ProxyScale::Quarter,
                quality: 80,
            }
            .fingerprint(),
            "dnxhr_lb-quarter-q80"
        );
    }

    #[test]
    fn codec_and_scale_identifiers_round_trip() {
        for codec in PROXY_CODECS {
            assert_eq!(ProxyCodec::parse(codec.as_str()), Some(codec));
            assert_eq!(codec.to_string(), codec.as_str());
            assert!(!codec.label().is_empty());
        }
        for scale in [ProxyScale::Half, ProxyScale::Quarter] {
            assert_eq!(ProxyScale::parse(scale.as_str()), Some(scale));
            assert_eq!(scale.to_string(), scale.as_str());
        }
        assert_eq!(ProxyCodec::parse("h264"), None);
        assert_eq!(ProxyScale::parse("third"), None);
    }

    #[test]
    fn file_names_are_derived_from_the_hash_and_the_options() {
        let hash = ContentHash::from_bytes([5_u8; 32]);
        let options = ProxyOptions::default();
        let name = Proxy::file_name(hash, options);
        assert!(name.starts_with(&hash.to_string()));
        assert!(name.ends_with(".mjpeg-half-q75.proxy.mov"), "{name}");
        assert!(Proxy::manifest_file_name(hash, options).ends_with(".mjpeg-half-q75.proxy.json"));
        let other = ProxyOptions {
            scale: ProxyScale::Quarter,
            ..options
        };
        assert_ne!(
            Proxy::file_name(hash, other),
            Proxy::file_name(hash, options)
        );
    }

    #[test]
    fn the_policy_triggers_only_above_its_threshold() {
        let policy = ProxyPolicy::default();
        assert!(policy.should_generate(&info("video/x-h264", 3840, 2160)));
        assert!(policy.should_generate(&info("video/x-h265", 4096, 2160)));
        // HD and below stays as it is: a proxy would cost more than it saves.
        assert!(!policy.should_generate(&info("video/x-h264", 1920, 1080)));
        assert!(!policy.should_generate(&info("video/x-h264", 1280, 720)));
        // Vertical footage crosses the threshold on height alone.
        assert!(policy.should_generate(&info("video/x-h264", 1080, 3840)));
        // Intra-only sources already scrub well.
        assert!(!policy.should_generate(&info("video/x-prores", 3840, 2160)));
        assert!(!policy.should_generate(&info("image/jpeg", 3840, 2160)));
        // A file with no video stream is never proxied.
        let mut audio_only = info("video/x-h264", 3840, 2160);
        audio_only.video.clear();
        assert!(!policy.should_generate(&audio_only));
    }

    #[test]
    fn the_threshold_and_the_intra_rule_are_configurable() {
        let lower = ProxyPolicy {
            min_width: 1280,
            min_height: 720,
            ..ProxyPolicy::default()
        };
        assert!(lower.should_generate(&info("video/x-h264", 1920, 1080)));
        assert!(!lower.should_generate(&info("video/x-h264", 1280, 720)));

        let intra_too = ProxyPolicy {
            include_intra_sources: true,
            ..ProxyPolicy::default()
        };
        assert!(intra_too.should_generate(&info("video/x-prores", 3840, 2160)));

        let off = ProxyPolicy {
            enabled: false,
            ..ProxyPolicy::default()
        };
        assert!(!off.should_generate(&info("video/x-h264", 3840, 2160)));
    }

    #[test]
    fn the_policy_hands_back_options_only_for_the_sources_it_triggers_on() {
        let policy = ProxyPolicy {
            scale: ProxyScale::Quarter,
            quality: 60,
            ..ProxyPolicy::default()
        };
        let options = policy
            .options_for(&info("video/x-h264", 3840, 2160))
            .expect("a 4K long-GOP source is proxied");
        assert_eq!(options.scale, ProxyScale::Quarter);
        assert_eq!(options.quality, 60);
        // Whichever codec this installation can write, it must be one this
        // module knows how to drive.
        assert!(PROXY_CODECS.contains(&options.codec));
        assert!(
            policy
                .options_for(&info("video/x-h264", 1920, 1080))
                .is_none()
        );
    }

    #[test]
    fn long_gop_is_assumed_unless_the_codec_is_known_to_be_intra_only() {
        assert!(is_long_gop("video/x-h264"));
        assert!(is_long_gop("video/x-h265"));
        assert!(is_long_gop("video/x-av1"));
        assert!(is_long_gop("video/x-vp9"));
        assert!(is_long_gop("video/x-something-new"));
        assert!(!is_long_gop("video/x-prores"));
        assert!(!is_long_gop("video/x-dnxhd"));
        assert!(!is_long_gop("image/jpeg"));
        assert!(!is_long_gop("IMAGE/JPEG"));
    }

    #[test]
    fn mjpeg_is_writable_on_every_installation() {
        assert!(
            ProxyCodec::Mjpeg.is_available_at(960, 540),
            "jpegenc and qtmux are part of gst-plugins-good"
        );
        assert_eq!(ProxyCodec::preferred_at(960, 540), {
            let preferred = ProxyCodec::preferred_at(960, 540);
            assert!(preferred.is_available_at(960, 540));
            preferred
        });
    }

    #[test]
    fn a_proxy_round_trips_through_its_manifest_and_a_broken_one_is_ignored() {
        let dir = std::env::temp_dir().join(format!("sub-proxy-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        let hash = ContentHash::from_bytes([4_u8; 32]);
        let options = ProxyOptions::default();
        let proxy = Proxy {
            cache_dir: dir.clone(),
            source_hash: hash,
            options,
            file: Proxy::file_name(hash, options),
            width: 960,
            height: 540,
            frame_count: 125,
        };

        // Without the file there is nothing to load, however good the
        // manifest is.
        proxy.write_manifest().expect("manifest");
        assert!(Proxy::load(&dir, hash, options).is_none());

        std::fs::write(proxy.path(), b"not really a movie").expect("proxy file");
        let loaded = Proxy::load(&dir, hash, options).expect("a complete proxy");
        assert_eq!(loaded, proxy);
        assert_eq!(loaded.frame_count(), 125);
        assert_eq!(loaded.width(), 960);
        assert_eq!(loaded.height(), 540);
        assert_eq!(loaded.cache_dir(), dir);
        assert_eq!(loaded.source_hash(), hash);
        assert_eq!(loaded.options(), options);

        // Other options, other bytes and a corrupt manifest all read as
        // "generate it again" rather than as an error.
        assert!(
            Proxy::load(
                &dir,
                hash,
                ProxyOptions {
                    scale: ProxyScale::Quarter,
                    ..options
                }
            )
            .is_none()
        );
        assert!(Proxy::load(&dir, ContentHash::from_bytes([9_u8; 32]), options).is_none());
        std::fs::write(
            dir.join(Proxy::manifest_file_name(hash, options)),
            b"{ not json",
        )
        .expect("corrupt");
        assert!(Proxy::load(&dir, hash, options).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generation_refuses_options_it_could_never_satisfy() {
        let dir = std::env::temp_dir().join(format!("sub-proxy-bad-{}", std::process::id()));
        let err = Proxy::generate(
            &dir.join("missing.mp4"),
            &dir,
            ProxyOptions {
                quality: 0,
                ..ProxyOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
    }
}
