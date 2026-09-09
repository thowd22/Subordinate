//! Thumbnail strips: the pictures the media bin and the timeline draw
//! (docs/PLAN.md §5.2).
//!
//! A strip is *N* evenly spaced pictures of one media item, each decoded once,
//! scaled down and written into the project's sidecar directory as a JPEG. The
//! work never happens on the UI or engine thread: it is submitted to the
//! [`JobService`](sub_core::JobService) with a [`Priority`], reports progress
//! as it goes, and stops promptly when it is cancelled.
//!
//! Everything about a strip is derived from the source's [`ContentHash`] and
//! its [`ThumbnailOptions`], so the file names of a strip are the same on
//! every machine and across runs. That is what makes the work resumable:
//! generation plans the whole strip up front, skips every frame whose file is
//! already on disk, and decodes only what is missing. A strip that was
//! finished before the application was last closed costs one hash and one
//! manifest read; a strip that was interrupted half way costs only its
//! remaining frames.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use std::path::Path;
//! use sub_core::{JobService, Priority};
//! use sub_media::{ThumbnailOptions, ThumbnailStrip, spawn_thumbnail_job};
//!
//! let jobs = JobService::with_default_workers();
//! let job = spawn_thumbnail_job(
//!     &jobs,
//!     Path::new("/media/take-1.mov"),
//!     Path::new("/projects/cut.sub.d"),
//!     ThumbnailOptions::default(),
//!     Priority::Normal,
//! );
//! let strip: ThumbnailStrip = job.wait()?;
//! assert_eq!(strip.frames().len(), ThumbnailOptions::default().count);
//! # Ok(())
//! # }
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sub_core::{
    CancelToken, JobContext, JobHandle, JobService, Priority, ResultExt, SubError, SubResult,
};
use sub_model::ContentHash;
use sub_time::RationalTime;

use crate::codes;
use crate::decode::{Decoder, DecoderOptions, FrameFormat, HardwarePreference, VideoFrame};
use crate::probe::{NANOSECONDS, probe};

/// Schema version of the strip manifest. A manifest written by an older
/// version is regenerated rather than trusted.
pub const THUMBNAIL_MANIFEST_VERSION: u32 = 1;

/// The name every thumbnail job is submitted under.
pub const THUMBNAIL_JOB_KIND: &str = "thumbnails";

/// Upper bound on the pictures in one strip. A strip is a UI affordance, not a
/// proxy: this bounds the decoding a hostile or mistaken caller can ask for.
pub const MAX_THUMBNAILS: usize = 512;

/// Upper bound on the width of one thumbnail.
pub const MAX_THUMBNAIL_WIDTH: u32 = 1920;

/// How a strip is generated.
///
/// The options are part of a strip's identity: two different option sets
/// produce two different sets of files, so changing them never leaves a caller
/// reading pictures made for something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThumbnailOptions {
    /// How many pictures the strip holds.
    pub count: usize,
    /// Width each picture is scaled down to, keeping the source aspect. A
    /// source narrower than this is not scaled up.
    pub max_width: u32,
    /// JPEG quality, 1..=100.
    pub quality: u8,
}

impl Default for ThumbnailOptions {
    fn default() -> Self {
        Self {
            count: 12,
            max_width: 320,
            quality: 80,
        }
    }
}

impl ThumbnailOptions {
    /// Rejects options no strip could be made from.
    ///
    /// # Errors
    ///
    /// Returns `core.invalid_argument` when the count or the width is zero or
    /// past its maximum, or when the quality is outside 1..=100.
    pub fn validate(self) -> SubResult<()> {
        let bad = |message: &str| {
            Err(
                SubError::new(sub_core::codes::INVALID_ARGUMENT, message.to_owned())
                    .with_detail("count", self.count)
                    .with_detail("max_width", self.max_width)
                    .with_detail("quality", self.quality),
            )
        };
        if self.count == 0 || self.count > MAX_THUMBNAILS {
            return bad("thumbnail count must be between 1 and 512");
        }
        if self.max_width == 0 || self.max_width > MAX_THUMBNAIL_WIDTH {
            return bad("thumbnail width must be between 1 and 1920");
        }
        if self.quality == 0 || self.quality > 100 {
            return bad("thumbnail quality must be between 1 and 100");
        }
        Ok(())
    }

    /// The part of a file name that distinguishes one option set from another.
    #[must_use]
    pub fn fingerprint(self) -> String {
        format!("t{}w{}q{}", self.count, self.max_width, self.quality)
    }
}

/// One picture of a strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThumbnailFrame {
    /// Position in the strip, `0` first.
    pub index: usize,
    /// The presentation timestamp the picture was asked for, in nanoseconds.
    pub pts_ns: i64,
    /// File name inside the sidecar directory.
    pub file: String,
    /// Width of the written picture in pixels.
    pub width: u32,
    /// Height of the written picture in pixels.
    pub height: u32,
}

impl ThumbnailFrame {
    /// The requested timestamp as an exact time in nanoseconds.
    #[must_use]
    pub fn pts(&self) -> RationalTime {
        RationalTime::new(self.pts_ns, NANOSECONDS)
    }

    /// Where the picture lives, given the sidecar directory.
    #[must_use]
    pub fn path(&self, cache_dir: &Path) -> PathBuf {
        cache_dir.join(&self.file)
    }
}

/// The on-disk manifest of a strip, and what [`ThumbnailStrip`] round-trips.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManifestFile {
    version: u32,
    source_hash: String,
    options: ThumbnailOptions,
    frames: Vec<ThumbnailFrame>,
}

/// A generated strip: where its pictures are and what times they show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailStrip {
    cache_dir: PathBuf,
    source_hash: ContentHash,
    options: ThumbnailOptions,
    frames: Vec<ThumbnailFrame>,
}

impl ThumbnailStrip {
    /// The sidecar directory holding the pictures.
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// The hash of the source these pictures were made from.
    #[must_use]
    pub fn source_hash(&self) -> ContentHash {
        self.source_hash
    }

    /// The options the strip was made with.
    #[must_use]
    pub fn options(&self) -> ThumbnailOptions {
        self.options
    }

    /// The pictures, in strip order.
    #[must_use]
    pub fn frames(&self) -> &[ThumbnailFrame] {
        &self.frames
    }

    /// The picture closest to `time`, for a timeline strip drawing a clip.
    #[must_use]
    pub fn frame_at(&self, time: RationalTime) -> Option<&ThumbnailFrame> {
        let wanted = time.rescaled_to(NANOSECONDS).value();
        self.frames
            .iter()
            .min_by_key(|frame| (frame.pts_ns - wanted).abs())
    }

    /// File name of the manifest of a strip.
    #[must_use]
    pub fn manifest_file_name(hash: ContentHash, options: ThumbnailOptions) -> String {
        format!("{hash}.{}.thumbs.json", options.fingerprint())
    }

    /// File name of one picture of a strip.
    #[must_use]
    pub fn frame_file_name(hash: ContentHash, options: ThumbnailOptions, index: usize) -> String {
        format!("{hash}.{}.{index:03}.jpg", options.fingerprint())
    }

    /// Reads a complete strip back from the sidecar directory.
    ///
    /// Returns `None` when there is no manifest, when it was written by
    /// another schema version, for other bytes or for other options, or when
    /// any picture it names is missing — every one of which is a reason to
    /// generate the strip again rather than to fail.
    #[must_use]
    pub fn load(cache_dir: &Path, hash: ContentHash, options: ThumbnailOptions) -> Option<Self> {
        let manifest_path = cache_dir.join(Self::manifest_file_name(hash, options));
        let text = std::fs::read_to_string(&manifest_path).ok()?;
        let manifest: ManifestFile = serde_json::from_str(&text).ok()?;
        if manifest.version != THUMBNAIL_MANIFEST_VERSION
            || manifest.options != options
            || manifest.source_hash != hash.to_string()
            || manifest.frames.len() != options.count
        {
            return None;
        }
        if !manifest
            .frames
            .iter()
            .all(|frame| is_written(&frame.path(cache_dir)))
        {
            return None;
        }
        Some(Self {
            cache_dir: cache_dir.to_path_buf(),
            source_hash: hash,
            options,
            frames: manifest.frames,
        })
    }

    /// Generates the strip for `path` into `cache_dir`, reusing whatever is
    /// already there.
    ///
    /// # Errors
    ///
    /// Returns `media.thumbnail_failed` when the source cannot be read,
    /// probed, decoded or encoded, and `core.invalid_argument` for options no
    /// strip could be made from.
    pub fn generate(
        path: &Path,
        cache_dir: &Path,
        options: ThumbnailOptions,
    ) -> SubResult<ThumbnailStrip> {
        Self::generate_with(
            path,
            cache_dir,
            options,
            &CancelToken::new(),
            &mut |_, _| {},
        )
    }

    /// Generates the strip, reporting progress and stopping when `cancel` is
    /// set.
    ///
    /// `progress` is called with `(finished, total)` after every picture,
    /// including the ones that were already on disk.
    ///
    /// # Errors
    ///
    /// As [`ThumbnailStrip::generate`], plus `core.cancelled` when the token
    /// is set before the strip is complete.
    pub fn generate_with(
        path: &Path,
        cache_dir: &Path,
        options: ThumbnailOptions,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(u64, u64),
    ) -> SubResult<ThumbnailStrip> {
        options.validate()?;
        let hash = hash_source(path)?;
        let total = options.count as u64;

        // The cheapest possible path: a finished strip needs no probe, no
        // pipeline and no decode.
        if let Some(strip) = Self::load(cache_dir, hash, options) {
            tracing::debug!(path = %path.display(), "thumbnail strip already generated");
            progress(total, total);
            return Ok(strip);
        }
        check_cancelled(cancel)?;

        let info = probe(path).map_err(|err| failed("media file cannot be probed", path, &err))?;
        let video = info.video.first().ok_or_else(|| {
            SubError::new(codes::NO_VIDEO_STREAM, "media file carries no video stream")
                .with_detail("path", path.display().to_string())
        })?;
        let duration = info.duration.ok_or_else(|| {
            SubError::new(
                codes::THUMBNAIL_FAILED,
                "media file declares no duration to spread thumbnails over",
            )
            .with_detail("path", path.display().to_string())
        })?;
        let (width, height) = thumbnail_size(video.width, video.height, options.max_width);
        let plan = strip_times(duration, options.count);

        let mut frames = Vec::with_capacity(options.count);
        let mut decoder: Option<Decoder> = None;
        for (index, pts) in plan.iter().enumerate() {
            check_cancelled(cancel)?;
            let file = Self::frame_file_name(hash, options, index);
            let frame = ThumbnailFrame {
                index,
                pts_ns: pts.value(),
                file,
                width,
                height,
            };
            let target = frame.path(cache_dir);
            if is_written(&target) {
                // Already generated, by an earlier run or an earlier session.
                frames.push(frame);
                progress(index as u64 + 1, total);
                continue;
            }
            let decoder = match &mut decoder {
                Some(decoder) => decoder,
                slot => slot.insert(open_decoder(path)?),
            };
            let picture = decoder
                .seek_to(*pts)
                .map_err(|err| failed("thumbnail frame cannot be decoded", path, &err))?
                .ok_or_else(|| {
                    SubError::new(
                        codes::THUMBNAIL_FAILED,
                        "no picture at the thumbnail timestamp",
                    )
                    .with_detail("path", path.display().to_string())
                    .with_detail("pts_ns", pts.value())
                })?;
            let rgb = to_rgb(&picture, width, height)?;
            write_jpeg(&target, &rgb, width, height, options.quality)?;
            frames.push(frame);
            progress(index as u64 + 1, total);
        }

        let strip = Self {
            cache_dir: cache_dir.to_path_buf(),
            source_hash: hash,
            options,
            frames,
        };
        strip.write_manifest()?;
        Ok(strip)
    }

    /// Writes the manifest beside the pictures, atomically.
    fn write_manifest(&self) -> SubResult<()> {
        let path = self
            .cache_dir
            .join(Self::manifest_file_name(self.source_hash, self.options));
        let manifest = ManifestFile {
            version: THUMBNAIL_MANIFEST_VERSION,
            source_hash: self.source_hash.to_string(),
            options: self.options,
            frames: self.frames.clone(),
        };
        let text = serde_json::to_string(&manifest).sub_context(
            codes::THUMBNAIL_FAILED,
            "thumbnail manifest cannot be serialised",
        )?;
        write_atomically(&path, text.as_bytes())
    }
}

/// A thumbnail job running on a [`JobService`].
///
/// The handle carries the job's state, its cancellation and its progress
/// events; [`ThumbnailJob::wait`] additionally hands back the strip itself.
#[derive(Debug, Clone)]
pub struct ThumbnailJob {
    handle: JobHandle,
    strip: Arc<Mutex<Option<ThumbnailStrip>>>,
}

impl ThumbnailJob {
    /// The underlying job handle: state, priority, cancellation.
    #[must_use]
    pub fn handle(&self) -> &JobHandle {
        &self.handle
    }

    /// Asks the job to stop; it does so between pictures.
    pub fn cancel(&self) {
        self.handle.cancel();
    }

    /// Blocks until the job finishes and returns the strip.
    ///
    /// # Errors
    ///
    /// Returns the job's error, or `core.cancelled` when it was cancelled.
    pub fn wait(&self) -> SubResult<ThumbnailStrip> {
        self.handle.wait().into_result()?;
        let strip = self
            .strip
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        strip.ok_or_else(|| {
            SubError::new(
                sub_core::codes::INTERNAL,
                "a completed thumbnail job left no strip",
            )
        })
    }
}

/// Queues thumbnail generation for `path` on `jobs`.
///
/// Returns at once; the strip is generated on a worker thread and progress
/// reaches [`JobService::subscribe`] subscribers as
/// [`JobEvent::Progress`](sub_core::JobEvent::Progress) with the finished and
/// total picture counts.
pub fn spawn_thumbnail_job(
    jobs: &JobService,
    path: &Path,
    cache_dir: &Path,
    options: ThumbnailOptions,
    priority: Priority,
) -> ThumbnailJob {
    let path = path.to_path_buf();
    let cache_dir = cache_dir.to_path_buf();
    let strip = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&strip);
    let handle = jobs.submit(THUMBNAIL_JOB_KIND, priority, move |ctx: &JobContext| {
        let cancel = ctx.cancel_token();
        let generated = ThumbnailStrip::generate_with(
            &path,
            &cache_dir,
            options,
            &cancel,
            &mut |done, total| ctx.progress(done, total),
        )?;
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(generated);
        Ok(())
    });
    ThumbnailJob { handle, strip }
}

/// The timestamps a strip of `count` pictures samples.
///
/// Each picture sits at the centre of its own slice of the source, so no
/// picture is the very first or the very last frame — the first frame of a
/// shot is often black, and the last is often past the last decodable
/// picture. The arithmetic is exact integer nanoseconds: nothing here divides
/// a duration as a float.
#[must_use]
pub fn strip_times(duration: RationalTime, count: usize) -> Vec<RationalTime> {
    let duration_ns = i128::from(duration.rescaled_to(NANOSECONDS).value().max(0));
    let count128 = i128::try_from(count).unwrap_or(i128::MAX);
    (0..count)
        .map(|index| {
            let numerator = duration_ns * (2 * i128::try_from(index).unwrap_or(0) + 1);
            let ns = numerator / (2 * count128.max(1));
            RationalTime::new(i64::try_from(ns).unwrap_or(i64::MAX), NANOSECONDS)
        })
        .collect()
}

/// The size one thumbnail is written at: `max_width` wide, keeping the source
/// aspect, and never scaled up past the source.
#[must_use]
pub fn thumbnail_size(source_width: u32, source_height: u32, max_width: u32) -> (u32, u32) {
    let source_width = source_width.max(1);
    let source_height = source_height.max(1);
    let width = max_width.clamp(1, source_width);
    let height = (u64::from(width) * u64::from(source_height) / u64::from(source_width)).max(1);
    (width, u32::try_from(height).unwrap_or(source_height))
}

/// Opens the decoder a strip is decoded with.
///
/// Software decoding is asked for: a strip is a handful of scattered seeks
/// over a file the user has just imported, where spinning up a hardware
/// decoder context costs more than it saves and where a driver quirk would
/// show up as a wrong picture in the bin.
fn open_decoder(path: &Path) -> SubResult<Decoder> {
    let options = DecoderOptions {
        hardware: HardwarePreference::Software,
        ..DecoderOptions::default()
    };
    Decoder::open_with(path, options)
        .map_err(|err| failed("media file cannot be opened for thumbnails", path, &err))
}

/// Converts a decoded picture to packed RGB at the thumbnail size.
fn to_rgb(frame: &VideoFrame, width: u32, height: u32) -> SubResult<Vec<u8>> {
    let missing = |plane: u32| {
        SubError::new(codes::THUMBNAIL_FAILED, "decoded frame is missing a plane")
            .with_detail("plane", plane)
    };
    let source = PlaneSize {
        width: frame.width() as usize,
        height: frame.height() as usize,
    };
    let target = PlaneSize {
        width: width as usize,
        height: height as usize,
    };
    let luma = Plane {
        data: frame.plane_data(0).ok_or_else(|| missing(0))?,
        stride: frame.plane_stride(0).ok_or_else(|| missing(0))? as usize,
    };
    let chroma = match frame.format() {
        FrameFormat::Nv12 => Chroma::Interleaved(Plane {
            data: frame.plane_data(1).ok_or_else(|| missing(1))?,
            stride: frame.plane_stride(1).ok_or_else(|| missing(1))? as usize,
        }),
        FrameFormat::I420 => Chroma::Planar(
            Plane {
                data: frame.plane_data(1).ok_or_else(|| missing(1))?,
                stride: frame.plane_stride(1).ok_or_else(|| missing(1))? as usize,
            },
            Plane {
                data: frame.plane_data(2).ok_or_else(|| missing(2))?,
                stride: frame.plane_stride(2).ok_or_else(|| missing(2))? as usize,
            },
        ),
    };
    Ok(scale_to_rgb(&luma, &chroma, source, target))
}

/// One mapped plane: its bytes and its row stride, which is never assumed to
/// equal the width.
struct Plane<'a> {
    data: &'a [u8],
    stride: usize,
}

impl Plane<'_> {
    /// One sample, or the mid-grey/neutral fallback when the coordinates fall
    /// outside the mapped bytes.
    fn sample(&self, x: usize, y: usize, fallback: u8) -> u8 {
        self.data
            .get(y * self.stride + x)
            .copied()
            .unwrap_or(fallback)
    }
}

/// How the chroma of a frame is laid out.
enum Chroma<'a> {
    /// NV12: one plane of interleaved Cb, Cr pairs at half resolution.
    Interleaved(Plane<'a>),
    /// I420: separate Cb and Cr planes at half resolution.
    Planar(Plane<'a>, Plane<'a>),
}

impl Chroma<'_> {
    /// The (Cb, Cr) pair covering the luma pixel at `(x, y)`.
    fn sample(&self, x: usize, y: usize) -> (u8, u8) {
        let (cx, cy) = (x / 2, y / 2);
        match self {
            Self::Interleaved(plane) => (
                plane.sample(cx * 2, cy, 128),
                plane.sample(cx * 2 + 1, cy, 128),
            ),
            Self::Planar(cb, cr) => (cb.sample(cx, cy, 128), cr.sample(cx, cy, 128)),
        }
    }
}

/// The pixel dimensions of a picture.
#[derive(Debug, Clone, Copy)]
struct PlaneSize {
    width: usize,
    height: usize,
}

/// Scales a 4:2:0 picture down to `target` and converts it to packed RGB.
///
/// Luma is box-averaged over the source pixels each target pixel covers, so a
/// downscale of a detailed picture does not alias into noise; chroma is point
/// sampled at the centre of that box, which at thumbnail sizes is
/// indistinguishable and a quarter of the work.
fn scale_to_rgb(
    luma: &Plane<'_>,
    chroma: &Chroma<'_>,
    source: PlaneSize,
    target: PlaneSize,
) -> Vec<u8> {
    let mut rgb = vec![0_u8; target.width * target.height * 3];
    if source.width == 0 || source.height == 0 {
        return rgb;
    }
    for ty in 0..target.height {
        let y0 = ty * source.height / target.height;
        let y1 = (((ty + 1) * source.height).div_ceil(target.height)).clamp(y0 + 1, source.height);
        for tx in 0..target.width {
            let x0 = tx * source.width / target.width;
            let x1 = (((tx + 1) * source.width).div_ceil(target.width)).clamp(x0 + 1, source.width);

            let mut sum = 0_u32;
            let mut samples = 0_u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += u32::from(luma.sample(x, y, 16));
                    samples += 1;
                }
            }
            let y_sample = u8::try_from(sum / samples.max(1)).unwrap_or(u8::MAX);
            let (cb, cr) = chroma.sample(usize::midpoint(x0, x1), usize::midpoint(y0, y1));

            let offset = (ty * target.width + tx) * 3;
            let [r, g, b] = ycbcr_to_rgb(y_sample, cb, cr);
            rgb[offset] = r;
            rgb[offset + 1] = g;
            rgb[offset + 2] = b;
        }
    }
    rgb
}

/// BT.709 limited-range `Y'CbCr` to full-range RGB, in integer arithmetic.
///
/// BT.709 is what the MVP assumes everywhere (decision-3: colour tags are
/// stored but not applied), and a thumbnail is the one place where being a
/// shade off matters least.
fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let c = i32::from(y) - 16;
    let d = i32::from(cb) - 128;
    let e = i32::from(cr) - 128;
    let clamp = |value: i32| u8::try_from(value.clamp(0, 255)).unwrap_or(u8::MAX);
    [
        clamp((298 * c + 459 * e + 128) >> 8),
        clamp((298 * c - 55 * d - 136 * e + 128) >> 8),
        clamp((298 * c + 541 * d + 128) >> 8),
    ]
}

/// Encodes packed RGB as a JPEG and writes it into the sidecar directory.
fn write_jpeg(path: &Path, rgb: &[u8], width: u32, height: u32, quality: u8) -> SubResult<()> {
    let (Ok(width16), Ok(height16)) = (u16::try_from(width), u16::try_from(height)) else {
        return Err(
            SubError::new(codes::THUMBNAIL_FAILED, "thumbnail is too large to encode")
                .with_detail("width", width)
                .with_detail("height", height),
        );
    };
    let mut encoded = Vec::new();
    jpeg_encoder::Encoder::new(&mut encoded, quality)
        .encode(rgb, width16, height16, jpeg_encoder::ColorType::Rgb)
        .sub_context_with(codes::THUMBNAIL_FAILED, || {
            format!("thumbnail {} cannot be encoded", path.display())
        })?;
    write_atomically(path, &encoded)
}

/// Writes `bytes` to `path` through a temporary file, creating the sidecar
/// directory if it is not there yet.
///
/// The rename means a reader never sees half a thumbnail, and it is what makes
/// [`is_written`] a sound test for "this frame is already generated".
fn write_atomically(path: &Path, bytes: &[u8]) -> SubResult<()> {
    let io = |err: &std::io::Error| {
        SubError::wrap(codes::THUMBNAIL_FAILED, "thumbnail cannot be written", err)
            .with_detail("path", path.display().to_string())
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| io(&err))?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes).map_err(|err| io(&err))?;
    std::fs::rename(&temporary, path).map_err(|err| io(&err))
}

/// Whether a file is there and holds something. A zero-length file is a
/// leftover, never a picture.
fn is_written(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

/// `Err(core.cancelled)` when the token is set.
fn check_cancelled(cancel: &CancelToken) -> SubResult<()> {
    if cancel.is_cancelled() {
        return Err(CancelToken::cancelled_error("the thumbnail job"));
    }
    Ok(())
}

/// Wraps a lower-level failure as this module's code, keeping the original
/// code as a detail so a caller can still tell a missing file from a broken
/// codec.
fn failed(message: &str, path: &Path, cause: &SubError) -> SubError {
    SubError::new(codes::THUMBNAIL_FAILED, message.to_owned())
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
            "media file cannot be read for thumbnails",
        )
        .with_detail("path", path.display().to_string())
        .with_cause(&err)
    })
}

#[cfg(test)]
mod tests {
    use sub_time::Rational;

    use super::*;

    /// A synthetic NV12 picture: a horizontal luma ramp with neutral chroma.
    fn nv12(width: usize, height: usize, stride: usize) -> (Vec<u8>, Vec<u8>) {
        let mut luma = vec![0_u8; stride * height];
        for y in 0..height {
            for x in 0..width {
                luma[y * stride + x] = u8::try_from((x * 219 / width.max(1)) + 16).unwrap_or(235);
            }
        }
        let chroma = vec![128_u8; stride * height.div_ceil(2)];
        (luma, chroma)
    }

    #[test]
    fn strip_times_are_exact_and_centred_in_their_slices() {
        let duration = RationalTime::new(1_000_000_000, NANOSECONDS);
        let times = strip_times(duration, 4);
        let values: Vec<i64> = times.iter().map(|t| t.value()).collect();
        assert_eq!(
            values,
            vec![125_000_000, 375_000_000, 625_000_000, 875_000_000]
        );
        for time in &times {
            assert_eq!(time.rate(), NANOSECONDS);
            assert!(*time < duration && !time.is_negative());
        }
    }

    #[test]
    fn strip_times_rescale_a_frame_rate_duration_without_drifting() {
        // 125 frames at 25 fps is exactly five seconds.
        let duration = RationalTime::new(125, Rational::FPS_25);
        let times = strip_times(duration, 5);
        assert_eq!(
            times.iter().map(|t| t.value()).collect::<Vec<_>>(),
            vec![
                500_000_000,
                1_500_000_000,
                2_500_000_000,
                3_500_000_000,
                4_500_000_000
            ]
        );
    }

    #[test]
    fn strip_times_handle_the_degenerate_cases() {
        assert!(strip_times(RationalTime::new(1000, NANOSECONDS), 0).is_empty());
        assert_eq!(strip_times(RationalTime::zero(NANOSECONDS), 3).len(), 3);
        assert!(
            strip_times(RationalTime::zero(NANOSECONDS), 3)
                .iter()
                .all(|time| time.is_zero())
        );
        // A negative duration is nonsense, and must not produce negative seeks.
        assert!(
            strip_times(RationalTime::new(-1000, NANOSECONDS), 2)
                .iter()
                .all(|t| !t.is_negative())
        );
    }

    #[test]
    fn thumbnail_size_keeps_the_aspect_and_never_upscales() {
        assert_eq!(thumbnail_size(1920, 1080, 320), (320, 180));
        assert_eq!(thumbnail_size(3840, 2160, 320), (320, 180));
        assert_eq!(thumbnail_size(640, 480, 320), (320, 240));
        assert_eq!(thumbnail_size(160, 120, 320), (160, 120));
        assert_eq!(thumbnail_size(0, 0, 320), (1, 1));
    }

    #[test]
    fn options_validate_their_bounds_and_fingerprint_themselves() {
        assert!(ThumbnailOptions::default().validate().is_ok());
        for bad in [
            ThumbnailOptions {
                count: 0,
                ..Default::default()
            },
            ThumbnailOptions {
                count: MAX_THUMBNAILS + 1,
                ..Default::default()
            },
            ThumbnailOptions {
                max_width: 0,
                ..Default::default()
            },
            ThumbnailOptions {
                max_width: MAX_THUMBNAIL_WIDTH + 1,
                ..Default::default()
            },
            ThumbnailOptions {
                quality: 0,
                ..Default::default()
            },
            ThumbnailOptions {
                quality: 101,
                ..Default::default()
            },
        ] {
            let err = bad.validate().unwrap_err();
            assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
        }
        assert_eq!(
            ThumbnailOptions {
                count: 8,
                max_width: 240,
                quality: 70
            }
            .fingerprint(),
            "t8w240q70"
        );
    }

    #[test]
    fn file_names_are_derived_from_the_hash_and_the_options() {
        let hash = ContentHash::from_bytes([7_u8; 32]);
        let options = ThumbnailOptions::default();
        let name = ThumbnailStrip::frame_file_name(hash, options, 7);
        assert!(name.starts_with(&hash.to_string()));
        assert!(name.ends_with(".t12w320q80.007.jpg"), "{name}");
        assert!(
            ThumbnailStrip::manifest_file_name(hash, options).ends_with(".t12w320q80.thumbs.json")
        );
        // Different options never share a file with these ones.
        let other = ThumbnailOptions {
            count: 8,
            ..options
        };
        assert_ne!(
            ThumbnailStrip::frame_file_name(hash, other, 7),
            ThumbnailStrip::frame_file_name(hash, options, 7)
        );
    }

    #[test]
    fn luma_is_box_averaged_and_a_stride_is_never_treated_as_a_width() {
        let (luma, chroma) = nv12(8, 4, 16);
        let rgb = scale_to_rgb(
            &Plane {
                data: &luma,
                stride: 16,
            },
            &Chroma::Interleaved(Plane {
                data: &chroma,
                stride: 16,
            }),
            PlaneSize {
                width: 8,
                height: 4,
            },
            PlaneSize {
                width: 4,
                height: 2,
            },
        );
        assert_eq!(rgb.len(), 4 * 2 * 3);
        // Neutral chroma means grey: the three channels agree.
        for pixel in rgb.chunks_exact(3) {
            assert!(
                pixel[0].abs_diff(pixel[1]) <= 2 && pixel[1].abs_diff(pixel[2]) <= 2,
                "expected grey, got {pixel:?}"
            );
        }
        // The ramp survives the downscale: left is darker than right.
        let row: Vec<u8> = rgb.chunks_exact(3).take(4).map(|p| p[0]).collect();
        assert!(row.windows(2).all(|w| w[0] < w[1]), "{row:?}");
    }

    #[test]
    fn the_colour_conversion_maps_the_limited_range_end_points() {
        assert_eq!(ycbcr_to_rgb(16, 128, 128), [0, 0, 0]);
        assert_eq!(ycbcr_to_rgb(235, 128, 128), [255, 255, 255]);
        let red = ycbcr_to_rgb(63, 102, 240);
        assert!(red[0] > 200 && red[1] < 60 && red[2] < 60, "{red:?}");
    }

    #[test]
    fn a_strip_round_trips_through_its_manifest_and_a_broken_one_is_ignored() {
        let dir = std::env::temp_dir().join(format!("sub-thumbs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        let hash = ContentHash::from_bytes([3_u8; 32]);
        let options = ThumbnailOptions {
            count: 2,
            max_width: 32,
            quality: 60,
        };
        let frames: Vec<ThumbnailFrame> = (0..2)
            .map(|index| ThumbnailFrame {
                index,
                pts_ns: 1_000 * (i64::try_from(index).unwrap_or(0) + 1),
                file: ThumbnailStrip::frame_file_name(hash, options, index),
                width: 32,
                height: 18,
            })
            .collect();
        let strip = ThumbnailStrip {
            cache_dir: dir.clone(),
            source_hash: hash,
            options,
            frames: frames.clone(),
        };

        // Without the pictures there is nothing to load, however good the
        // manifest is.
        strip.write_manifest().expect("manifest");
        assert!(ThumbnailStrip::load(&dir, hash, options).is_none());

        for frame in &frames {
            std::fs::write(frame.path(&dir), b"not really a jpeg").expect("picture");
        }
        let loaded = ThumbnailStrip::load(&dir, hash, options).expect("a complete strip");
        assert_eq!(loaded, strip);
        assert_eq!(loaded.frames()[1].pts().value(), 2_000);
        assert_eq!(
            loaded
                .frame_at(RationalTime::new(1_400, NANOSECONDS))
                .map(|f| f.index),
            Some(0)
        );

        // Other options, other bytes and a corrupt manifest all read as
        // "generate it again" rather than as an error.
        assert!(
            ThumbnailStrip::load(
                &dir,
                hash,
                ThumbnailOptions {
                    count: 4,
                    ..options
                }
            )
            .is_none()
        );
        assert!(ThumbnailStrip::load(&dir, ContentHash::from_bytes([9_u8; 32]), options).is_none());
        std::fs::write(
            dir.join(ThumbnailStrip::manifest_file_name(hash, options)),
            b"{ not json",
        )
        .expect("corrupt");
        assert!(ThumbnailStrip::load(&dir, hash, options).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_written_thumbnail_is_a_readable_jpeg() {
        let dir = std::env::temp_dir().join(format!("sub-thumbs-jpeg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("frame.jpg");
        let rgb = vec![200_u8; 16 * 9 * 3];
        write_jpeg(&path, &rgb, 16, 9, 80).expect("a written jpeg");
        let bytes = std::fs::read(&path).expect("the file");
        assert!(bytes.len() > 2);
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG start-of-image marker");
        assert!(is_written(&path));
        assert!(!is_written(&dir.join("missing.jpg")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
