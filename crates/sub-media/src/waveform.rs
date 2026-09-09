//! Waveform peaks: the shape an audio clip is edited by (docs/PLAN.md §5.2).
//!
//! A waveform is a pyramid of min/max peaks over one media item's audio. The
//! base level summarises a fixed number of audio frames per peak; every
//! coarser level merges a fixed number of the level below it, so one media
//! item carries several zoom levels and the timeline can pick the one whose
//! peaks land about one per pixel instead of scanning the samples again.
//!
//! Like [`ThumbnailStrip`](crate::ThumbnailStrip), the work never happens on
//! the UI or engine thread: it is submitted to the
//! [`JobService`](sub_core::JobService) with a [`Priority`], reports progress
//! as it goes, and stops promptly when it is cancelled. Everything about a
//! waveform is derived from the source's [`ContentHash`] and its
//! [`WaveformOptions`], so the file names are the same on every machine and
//! across runs — which is what makes generation resumable. A run that was
//! interrupted keeps every level it had already written; a resumed run reads
//! those back, and when the base level is among them it derives the rest with
//! no decoding at all.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use std::path::Path;
//! use sub_core::{JobService, Priority};
//! use sub_media::{Waveform, WaveformOptions, spawn_waveform_job};
//!
//! let jobs = JobService::with_default_workers();
//! let job = spawn_waveform_job(
//!     &jobs,
//!     Path::new("/media/take-1.wav"),
//!     Path::new("/projects/cut.sub.d"),
//!     WaveformOptions::default(),
//!     Priority::Normal,
//! );
//! let waveform: Waveform = job.wait()?;
//! let level = waveform.level_for(1_024).expect("a level for this zoom");
//! let peaks = waveform.read_peaks(level.index)?;
//! assert_eq!(peaks.len(), level.peaks as usize * waveform.channels() as usize);
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
use sub_time::{Rational, RationalTime};

use crate::codes;
use crate::decode::{Decoder, DecoderOptions, HardwarePreference, StreamSelection};

/// Schema version of the waveform manifest. A manifest written by an older
/// version is regenerated rather than trusted.
pub const WAVEFORM_MANIFEST_VERSION: u32 = 1;

/// The name every waveform job is submitted under.
pub const WAVEFORM_JOB_KIND: &str = "waveforms";

/// The finest base resolution a waveform may be asked for, in audio frames
/// per peak. Below this a peak file costs more than the samples it summarises.
pub const MIN_FRAMES_PER_PEAK: u32 = 8;

/// The coarsest base resolution a waveform may be asked for.
pub const MAX_FRAMES_PER_PEAK: u32 = 1 << 16;

/// Upper bound on the levels of one pyramid. Each level is a whole file, and
/// a dozen of them already spans four orders of magnitude of zoom.
pub const MAX_WAVEFORM_LEVELS: u32 = 12;

/// The largest step between two levels of the pyramid.
pub const MAX_WAVEFORM_DECIMATION: u32 = 16;

/// How a waveform is generated.
///
/// The options are part of a waveform's identity: two different option sets
/// produce two different sets of files, so changing them never leaves a caller
/// reading peaks made for something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WaveformOptions {
    /// Audio frames summarised by one peak of the base level.
    pub frames_per_peak: u32,
    /// How many levels the pyramid holds, the base level included.
    pub levels: u32,
    /// How many peaks of one level are merged into one peak of the next.
    pub decimation: u32,
}

impl Default for WaveformOptions {
    fn default() -> Self {
        // 256 frames is about 5 ms at 48 kHz, and six levels stepping by four
        // reach 262 144 frames — five and a half seconds — a peak, which spans
        // the timeline's zoom ladder from single frames to a whole feature.
        Self {
            frames_per_peak: 256,
            levels: 6,
            decimation: 4,
        }
    }
}

impl WaveformOptions {
    /// Rejects options no waveform could be made from.
    ///
    /// # Errors
    ///
    /// Returns `core.invalid_argument` when the base resolution is outside
    /// [`MIN_FRAMES_PER_PEAK`]..=[`MAX_FRAMES_PER_PEAK`], when the level count
    /// is zero or past [`MAX_WAVEFORM_LEVELS`], or when the decimation is
    /// below two or past [`MAX_WAVEFORM_DECIMATION`].
    pub fn validate(self) -> SubResult<()> {
        let bad = |message: &str| {
            Err(
                SubError::new(sub_core::codes::INVALID_ARGUMENT, message.to_owned())
                    .with_detail("frames_per_peak", self.frames_per_peak)
                    .with_detail("levels", self.levels)
                    .with_detail("decimation", self.decimation),
            )
        };
        if self.frames_per_peak < MIN_FRAMES_PER_PEAK || self.frames_per_peak > MAX_FRAMES_PER_PEAK
        {
            return bad("waveform frames per peak must be between 8 and 65536");
        }
        if self.levels == 0 || self.levels > MAX_WAVEFORM_LEVELS {
            return bad("waveform level count must be between 1 and 12");
        }
        if self.decimation < 2 || self.decimation > MAX_WAVEFORM_DECIMATION {
            return bad("waveform decimation must be between 2 and 16");
        }
        Ok(())
    }

    /// Audio frames one peak of `level` summarises.
    ///
    /// Saturates rather than overflowing: the validated bounds keep every
    /// level well inside a `u64`, and a level past the last one is clamped to
    /// the last.
    #[must_use]
    pub fn frames_per_peak_at(self, level: u32) -> u64 {
        let level = level.min(self.levels.saturating_sub(1));
        u64::from(self.frames_per_peak)
            .saturating_mul(u64::from(self.decimation).saturating_pow(level))
    }

    /// The part of a file name that distinguishes one option set from another.
    #[must_use]
    pub fn fingerprint(self) -> String {
        format!(
            "p{}d{}l{}",
            self.frames_per_peak, self.decimation, self.levels
        )
    }
}

/// The loudest excursion, either way, of the samples one peak summarises.
///
/// Samples are quantised to `i16` on the way in: a waveform is a picture, and
/// sixteen bits is both more precision than a strip a hundred pixels tall can
/// show and half the bytes of an `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Peak {
    /// The most negative sample, quantised.
    pub min: i16,
    /// The most positive sample, quantised.
    pub max: i16,
}

impl Peak {
    /// A peak of no signal at all, and the identity of [`Peak::merged`].
    pub const SILENCE: Self = Self { min: 0, max: 0 };

    /// Bytes one peak occupies in a level file.
    pub const BYTES: usize = 4;

    /// The peak covering both `self` and `other`.
    #[must_use]
    pub fn merged(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// The peak widened to include one sample in `[-1.0, 1.0]`.
    #[must_use]
    pub fn with_sample(self, sample: f32) -> Self {
        let value = quantise(sample);
        Self {
            min: self.min.min(value),
            max: self.max.max(value),
        }
    }

    /// The most negative sample as a fraction of full scale.
    #[must_use]
    pub fn min_f32(self) -> f32 {
        f32::from(self.min) / f32::from(i16::MAX)
    }

    /// The most positive sample as a fraction of full scale.
    #[must_use]
    pub fn max_f32(self) -> f32 {
        f32::from(self.max) / f32::from(i16::MAX)
    }

    /// The peak's bytes in a level file: min then max, little-endian.
    #[must_use]
    pub fn to_le_bytes(self) -> [u8; Self::BYTES] {
        let min = self.min.to_le_bytes();
        let max = self.max.to_le_bytes();
        [min[0], min[1], max[0], max[1]]
    }

    /// A peak read back from a level file.
    #[must_use]
    pub fn from_le_bytes(bytes: [u8; Self::BYTES]) -> Self {
        Self {
            min: i16::from_le_bytes([bytes[0], bytes[1]]),
            max: i16::from_le_bytes([bytes[2], bytes[3]]),
        }
    }
}

/// One level of a waveform pyramid, as the manifest records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaveformLevel {
    /// Position in the pyramid, `0` the finest.
    pub index: u32,
    /// Audio frames one peak of this level summarises.
    pub frames_per_peak: u64,
    /// Peaks per channel this level holds.
    pub peaks: u64,
    /// File name inside the sidecar directory.
    pub file: String,
}

impl WaveformLevel {
    /// Where this level's peaks live, given the sidecar directory.
    #[must_use]
    pub fn path(&self, cache_dir: &Path) -> PathBuf {
        cache_dir.join(&self.file)
    }

    /// Bytes the level's file holds when it is complete.
    #[must_use]
    pub fn byte_len(&self, channels: u16) -> u64 {
        self.peaks
            .saturating_mul(u64::from(channels))
            .saturating_mul(Peak::BYTES as u64)
    }
}

/// The on-disk manifest of a waveform, and what [`Waveform`] round-trips.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManifestFile {
    version: u32,
    source_hash: String,
    options: WaveformOptions,
    sample_rate: u32,
    channels: u16,
    frames: u64,
    levels: Vec<WaveformLevel>,
}

/// A generated waveform: what its levels are and where their peaks live.
///
/// The peaks themselves stay on disk until a caller asks for them with
/// [`Waveform::read_peaks`]; the timeline reads one level, turns it into a
/// texture and keeps that instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waveform {
    cache_dir: PathBuf,
    source_hash: ContentHash,
    options: WaveformOptions,
    sample_rate: u32,
    channels: u16,
    frames: u64,
    levels: Vec<WaveformLevel>,
}

impl Waveform {
    /// The sidecar directory holding the peak files.
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// The hash of the source these peaks were made from.
    #[must_use]
    pub fn source_hash(&self) -> ContentHash {
        self.source_hash
    }

    /// The options the waveform was made with.
    #[must_use]
    pub fn options(&self) -> WaveformOptions {
        self.options
    }

    /// Sample rate of the source audio, in hertz.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channels each peak file interleaves.
    #[must_use]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Audio frames the source carries.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The source's audio duration, exact at its own sample rate.
    ///
    /// # Panics
    ///
    /// Never: a waveform is only ever built from a negotiated stream, whose
    /// sample rate is non-zero.
    #[must_use]
    pub fn duration(&self) -> RationalTime {
        let rate = Rational::new(self.sample_rate.max(1), 1).expect("a non-zero sample rate");
        RationalTime::new(i64::try_from(self.frames).unwrap_or(i64::MAX), rate)
    }

    /// The levels of the pyramid, finest first.
    #[must_use]
    pub fn levels(&self) -> &[WaveformLevel] {
        &self.levels
    }

    /// One level by its index.
    #[must_use]
    pub fn level(&self, index: u32) -> Option<&WaveformLevel> {
        self.levels.iter().find(|level| level.index == index)
    }

    /// The level to draw when one pixel covers `frames_per_pixel` audio
    /// frames.
    ///
    /// The coarsest level that still puts at least one peak in every pixel:
    /// drawing a finer one would read peaks the strip cannot show. A zoom
    /// finer than the base level gets the base level, which is as much detail
    /// as was ever stored.
    #[must_use]
    pub fn level_for(&self, frames_per_pixel: u64) -> Option<&WaveformLevel> {
        let coarsest = self
            .levels
            .iter()
            .filter(|level| level.frames_per_peak <= frames_per_pixel.max(1))
            .max_by_key(|level| level.frames_per_peak);
        coarsest.or_else(|| self.levels.first())
    }

    /// File name of the manifest of a waveform.
    #[must_use]
    pub fn manifest_file_name(hash: ContentHash, options: WaveformOptions) -> String {
        format!("{hash}.{}.wave.json", options.fingerprint())
    }

    /// File name of one level of a waveform.
    #[must_use]
    pub fn level_file_name(hash: ContentHash, options: WaveformOptions, level: u32) -> String {
        format!("{hash}.{}.{level:02}.peaks", options.fingerprint())
    }

    /// Reads a complete waveform back from the sidecar directory.
    ///
    /// Returns `None` when there is no manifest, when it was written by
    /// another schema version, for other bytes or for other options, or when
    /// any level it names is missing or the wrong length — every one of which
    /// is a reason to generate rather than to fail.
    #[must_use]
    pub fn load(cache_dir: &Path, hash: ContentHash, options: WaveformOptions) -> Option<Self> {
        let waveform = Self::load_manifest(cache_dir, hash, options)?;
        // A manifest can describe a pyramid that is still being built: it is
        // written after every level so an interrupted run can be resumed from
        // it. Only the whole pyramid counts as generated.
        (waveform.levels.len() == options.levels as usize
            && waveform
                .levels
                .iter()
                .all(|level| waveform.is_level_written(level)))
        .then_some(waveform)
    }

    /// Reads the peaks of one level, interleaved by channel: the peak of
    /// channel `c` in bucket `b` sits at `b * channels + c`.
    ///
    /// # Errors
    ///
    /// Returns `media.waveform_failed` when the level is not part of this
    /// waveform, or when its file is missing, unreadable or the wrong length.
    pub fn read_peaks(&self, level: u32) -> SubResult<Vec<Peak>> {
        let level = self.level(level).ok_or_else(|| {
            SubError::new(codes::WAVEFORM_FAILED, "no such waveform level")
                .with_detail("level", level)
        })?;
        read_peak_file(
            &level.path(&self.cache_dir),
            usize::try_from(level.peaks.saturating_mul(u64::from(self.channels)))
                .unwrap_or(usize::MAX),
        )
    }

    /// Generates the waveform for `path` into `cache_dir`, reusing whatever is
    /// already there.
    ///
    /// # Errors
    ///
    /// Returns `media.waveform_failed` when the source cannot be read, probed
    /// or decoded or the peaks cannot be written, `media.no_audio_stream` when
    /// the file carries no audio, and `core.invalid_argument` for options no
    /// waveform could be made from.
    pub fn generate(
        path: &Path,
        cache_dir: &Path,
        options: WaveformOptions,
    ) -> SubResult<Waveform> {
        Self::generate_with(
            path,
            cache_dir,
            options,
            &CancelToken::new(),
            &mut |_, _| {},
        )
    }

    /// Generates the waveform, reporting progress and stopping when `cancel`
    /// is set.
    ///
    /// `progress` is called with `(finished, total)` over the stages of the
    /// work: decoding the source into the base level, then each coarser level
    /// derived from the one below it.
    ///
    /// # Errors
    ///
    /// As [`Waveform::generate`], plus `core.cancelled` when the token is set
    /// before the pyramid is complete.
    pub fn generate_with(
        path: &Path,
        cache_dir: &Path,
        options: WaveformOptions,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(u64, u64),
    ) -> SubResult<Waveform> {
        options.validate()?;
        let hash = hash_source(path)?;
        let total = u64::from(options.levels);

        // The cheapest possible path: a finished waveform needs no pipeline
        // and no decode.
        if let Some(waveform) = Self::load(cache_dir, hash, options) {
            tracing::debug!(path = %path.display(), "waveform already generated");
            progress(total, total);
            return Ok(waveform);
        }
        check_cancelled(cancel)?;

        // An interrupted run leaves its manifest and whatever levels it had
        // written. When the base level is one of them the samples never have
        // to be decoded again.
        let resumed = Self::load_manifest(cache_dir, hash, options)
            .and_then(|waveform| waveform.resume_base().map(|peaks| (waveform, peaks)));
        let (mut waveform, mut peaks) = match resumed {
            Some((waveform, peaks)) => {
                tracing::debug!(path = %path.display(), "waveform base level resumed");
                (waveform, peaks)
            }
            None => Self::decode_base(path, cache_dir, hash, options, cancel)?,
        };
        progress(1, total);

        // Every coarser level is a decimation of the one below it, so the rest
        // of the pyramid costs no decoding whatever it was interrupted doing.
        for index in 1..options.levels {
            check_cancelled(cancel)?;
            peaks = decimate(&peaks, usize::from(waveform.channels), options.decimation);
            let level = waveform.plan_level(index, &peaks);
            if !waveform.is_level_written(&level) {
                write_peak_file(&level.path(cache_dir), &peaks)?;
            }
            waveform.set_level(level);
            waveform.write_manifest()?;
            progress(u64::from(index) + 1, total);
        }
        Ok(waveform)
    }

    /// Decodes the source's audio into the base level and writes it.
    fn decode_base(
        path: &Path,
        cache_dir: &Path,
        hash: ContentHash,
        options: WaveformOptions,
        cancel: &CancelToken,
    ) -> SubResult<(Self, Vec<Peak>)> {
        let mut decoder = open_decoder(path)?;
        let format = decoder
            .audio_format()
            .ok_or_else(|| {
                SubError::new(
                    codes::NO_AUDIO_STREAM,
                    "media file carries no audio stream to draw",
                )
                .with_detail("path", path.display().to_string())
            })?
            .clone();
        let mut builder = PeakBuilder::new(usize::from(format.channels), options.frames_per_peak);
        loop {
            check_cancelled(cancel)?;
            let Some(block) = decoder
                .next_audio_block()
                .map_err(|err| failed("audio cannot be decoded for a waveform", path, &err))?
            else {
                break;
            };
            let start = u64::try_from(block.start.value()).unwrap_or(0);
            builder.push_block(start, block.samples);
        }
        let (peaks, frames) = builder.finish();
        let mut waveform = Self {
            cache_dir: cache_dir.to_path_buf(),
            source_hash: hash,
            options,
            sample_rate: format.sample_rate,
            channels: format.channels,
            frames,
            levels: Vec::new(),
        };
        let level = waveform.plan_level(0, &peaks);
        write_peak_file(&level.path(cache_dir), &peaks)?;
        waveform.set_level(level);
        waveform.write_manifest()?;
        Ok((waveform, peaks))
    }

    /// Reads the manifest back, without checking that every level it plans is
    /// on disk. `None` for a manifest that describes something else.
    fn load_manifest(
        cache_dir: &Path,
        hash: ContentHash,
        options: WaveformOptions,
    ) -> Option<Self> {
        let manifest_path = cache_dir.join(Self::manifest_file_name(hash, options));
        let text = std::fs::read_to_string(&manifest_path).ok()?;
        let manifest: ManifestFile = serde_json::from_str(&text).ok()?;
        if manifest.version != WAVEFORM_MANIFEST_VERSION
            || manifest.options != options
            || manifest.source_hash != hash.to_string()
            || manifest.sample_rate == 0
            || manifest.channels == 0
            || manifest.levels.len() > options.levels as usize
        {
            return None;
        }
        Some(Self {
            cache_dir: cache_dir.to_path_buf(),
            source_hash: hash,
            options,
            sample_rate: manifest.sample_rate,
            channels: manifest.channels,
            frames: manifest.frames,
            levels: manifest.levels,
        })
    }

    /// The base level's peaks, when an earlier run left a complete file.
    fn resume_base(&self) -> Option<Vec<Peak>> {
        let level = self.level(0)?;
        if !self.is_level_written(level) {
            return None;
        }
        self.read_peaks(0).ok()
    }

    /// The manifest entry for `index`, given the peaks it holds.
    fn plan_level(&self, index: u32, peaks: &[Peak]) -> WaveformLevel {
        let channels = usize::from(self.channels).max(1);
        WaveformLevel {
            index,
            frames_per_peak: self.options.frames_per_peak_at(index),
            peaks: (peaks.len() / channels) as u64,
            file: Self::level_file_name(self.source_hash, self.options, index),
        }
    }

    /// Records a generated level, replacing any earlier plan for it.
    fn set_level(&mut self, level: WaveformLevel) {
        if let Some(existing) = self
            .levels
            .iter_mut()
            .find(|existing| existing.index == level.index)
        {
            *existing = level;
        } else {
            self.levels.push(level);
            self.levels.sort_by_key(|level| level.index);
        }
    }

    /// Whether a level's file is on disk at exactly the length it must have.
    ///
    /// A shorter file is a half-written leftover and a longer one was written
    /// for something else; either way it is generated again rather than read.
    fn is_level_written(&self, level: &WaveformLevel) -> bool {
        std::fs::metadata(level.path(&self.cache_dir))
            .is_ok_and(|meta| meta.is_file() && meta.len() == level.byte_len(self.channels))
    }

    /// Writes the manifest beside the peaks, atomically.
    fn write_manifest(&self) -> SubResult<()> {
        let path = self
            .cache_dir
            .join(Self::manifest_file_name(self.source_hash, self.options));
        let manifest = ManifestFile {
            version: WAVEFORM_MANIFEST_VERSION,
            source_hash: self.source_hash.to_string(),
            options: self.options,
            sample_rate: self.sample_rate,
            channels: self.channels,
            frames: self.frames,
            levels: self.levels.clone(),
        };
        let text = serde_json::to_string(&manifest).sub_context(
            codes::WAVEFORM_FAILED,
            "waveform manifest cannot be serialised",
        )?;
        write_atomically(&path, text.as_bytes())
    }
}

/// A waveform job running on a [`JobService`].
///
/// The handle carries the job's state, its cancellation and its progress
/// events; [`WaveformJob::wait`] additionally hands back the waveform itself.
#[derive(Debug, Clone)]
pub struct WaveformJob {
    handle: JobHandle,
    waveform: Arc<Mutex<Option<Waveform>>>,
}

impl WaveformJob {
    /// The underlying job handle: state, priority, cancellation.
    #[must_use]
    pub fn handle(&self) -> &JobHandle {
        &self.handle
    }

    /// Asks the job to stop; it does so between audio blocks and between
    /// levels.
    pub fn cancel(&self) {
        self.handle.cancel();
    }

    /// Blocks until the job finishes and returns the waveform.
    ///
    /// # Errors
    ///
    /// Returns the job's error, or `core.cancelled` when it was cancelled.
    pub fn wait(&self) -> SubResult<Waveform> {
        self.handle.wait().into_result()?;
        let waveform = self
            .waveform
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        waveform.ok_or_else(|| {
            SubError::new(
                sub_core::codes::INTERNAL,
                "a completed waveform job left no waveform",
            )
        })
    }
}

/// Queues waveform generation for `path` on `jobs`.
///
/// Returns at once; the peaks are computed on a worker thread and progress
/// reaches [`JobService::subscribe`] subscribers as
/// [`JobEvent::Progress`](sub_core::JobEvent::Progress) with the finished and
/// total level counts.
pub fn spawn_waveform_job(
    jobs: &JobService,
    path: &Path,
    cache_dir: &Path,
    options: WaveformOptions,
    priority: Priority,
) -> WaveformJob {
    let path = path.to_path_buf();
    let cache_dir = cache_dir.to_path_buf();
    let waveform = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&waveform);
    let handle = jobs.submit(WAVEFORM_JOB_KIND, priority, move |ctx: &JobContext| {
        let cancel = ctx.cancel_token();
        let generated =
            Waveform::generate_with(&path, &cache_dir, options, &cancel, &mut |done, total| {
                ctx.progress(done, total);
            })?;
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(generated);
        Ok(())
    });
    WaveformJob { handle, waveform }
}

/// Folds decoded audio blocks into the peaks of one level.
///
/// Peaks are placed by absolute audio frame position rather than by the order
/// blocks arrive in, so a gap in the stream leaves silence at exactly the
/// right place instead of shifting everything after it.
#[derive(Debug)]
struct PeakBuilder {
    channels: usize,
    frames_per_peak: u64,
    peaks: Vec<Peak>,
    frames: u64,
}

impl PeakBuilder {
    /// A builder for `channels` interleaved channels at `frames_per_peak`.
    fn new(channels: usize, frames_per_peak: u32) -> Self {
        Self {
            channels: channels.max(1),
            frames_per_peak: u64::from(frames_per_peak).max(1),
            peaks: Vec::new(),
            frames: 0,
        }
    }

    /// Folds one block of interleaved samples starting at audio frame
    /// `start`.
    fn push_block(&mut self, start: u64, samples: &[f32]) {
        let frames = samples.len() / self.channels;
        for frame in 0..frames {
            let bucket = (start.saturating_add(frame as u64)) / self.frames_per_peak;
            let base = usize::try_from(bucket)
                .unwrap_or(usize::MAX)
                .saturating_mul(self.channels);
            if self.peaks.len() < base + self.channels {
                self.peaks.resize(base + self.channels, Peak::SILENCE);
            }
            for channel in 0..self.channels {
                let sample = samples[frame * self.channels + channel];
                self.peaks[base + channel] = self.peaks[base + channel].with_sample(sample);
            }
        }
        self.frames = self.frames.max(start.saturating_add(frames as u64));
    }

    /// The peaks and the audio frames they cover.
    fn finish(self) -> (Vec<Peak>, u64) {
        (self.peaks, self.frames)
    }
}

/// Merges every `factor` peaks of `peaks` into one, per channel.
///
/// This is what makes the pyramid cheap: only the base level ever looks at a
/// sample, and a coarser level is a pass over the level below it.
#[must_use]
fn decimate(peaks: &[Peak], channels: usize, factor: u32) -> Vec<Peak> {
    let channels = channels.max(1);
    let factor = usize::try_from(factor).unwrap_or(1).max(1);
    let buckets = (peaks.len() / channels).div_ceil(factor);
    let mut out = vec![Peak::SILENCE; buckets * channels];
    for (index, peak) in peaks.iter().enumerate() {
        let bucket = (index / channels) / factor;
        let channel = index % channels;
        let slot = &mut out[bucket * channels + channel];
        *slot = slot.merged(*peak);
    }
    out
}

/// One sample in `[-1.0, 1.0]` as a quantised peak value.
fn quantise(sample: f32) -> i16 {
    let scaled = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round();
    // The clamp bounds `scaled` to ±32767, which every `i16` holds exactly.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the clamp above bounds the value to i16::MAX"
    )]
    {
        scaled as i16
    }
}

/// Opens the decoder a waveform is built from.
///
/// Audio only, and in software: no picture is ever looked at, and a hardware
/// video decoder context would be spun up for nothing.
fn open_decoder(path: &Path) -> SubResult<Decoder> {
    let options = DecoderOptions {
        streams: StreamSelection::AudioOnly,
        hardware: HardwarePreference::Software,
        ..DecoderOptions::default()
    };
    Decoder::open_with(path, options)
        .map_err(|err| failed("media file cannot be opened for a waveform", path, &err))
}

/// Writes a level's peaks into the sidecar directory, atomically.
fn write_peak_file(path: &Path, peaks: &[Peak]) -> SubResult<()> {
    let mut bytes = Vec::with_capacity(peaks.len() * Peak::BYTES);
    for peak in peaks {
        bytes.extend_from_slice(&peak.to_le_bytes());
    }
    write_atomically(path, &bytes)
}

/// Reads exactly `expected` peaks back from a level file.
fn read_peak_file(path: &Path, expected: usize) -> SubResult<Vec<Peak>> {
    let bytes = std::fs::read(path).map_err(|err| {
        SubError::wrap(
            codes::WAVEFORM_FAILED,
            "waveform peaks cannot be read",
            &err,
        )
        .with_detail("path", path.display().to_string())
    })?;
    if bytes.len() != expected * Peak::BYTES {
        return Err(SubError::new(
            codes::WAVEFORM_FAILED,
            "waveform peak file has the wrong length",
        )
        .with_detail("path", path.display().to_string())
        .with_detail("bytes", bytes.len())
        .with_detail("expected", expected * Peak::BYTES));
    }
    Ok(bytes
        .chunks_exact(Peak::BYTES)
        .map(|chunk| Peak::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

/// Writes `bytes` to `path` through a temporary file, creating the sidecar
/// directory if it is not there yet.
///
/// The rename means a reader never sees half a level, which is what makes a
/// length check a sound test for "this level is already generated".
fn write_atomically(path: &Path, bytes: &[u8]) -> SubResult<()> {
    let io = |err: &std::io::Error| {
        SubError::wrap(codes::WAVEFORM_FAILED, "waveform cannot be written", err)
            .with_detail("path", path.display().to_string())
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| io(&err))?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes).map_err(|err| io(&err))?;
    std::fs::rename(&temporary, path).map_err(|err| io(&err))
}

/// `Err(core.cancelled)` when the token is set.
fn check_cancelled(cancel: &CancelToken) -> SubResult<()> {
    if cancel.is_cancelled() {
        return Err(CancelToken::cancelled_error("the waveform job"));
    }
    Ok(())
}

/// Wraps a lower-level failure as this module's code, keeping the original
/// code as a detail so a caller can still tell a missing file from a broken
/// codec.
fn failed(message: &str, path: &Path, cause: &SubError) -> SubError {
    SubError::new(codes::WAVEFORM_FAILED, message.to_owned())
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
            "media file cannot be read for a waveform",
        )
        .with_detail("path", path.display().to_string())
        .with_cause(&err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ramp of `frames` stereo frames: the left channel climbs from 0 to 1,
    /// the right is its negation.
    fn ramp(frames: u16) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                let value = f32::from(frame) / f32::from(frames);
                [value, -value]
            })
            .collect()
    }

    #[test]
    fn options_reject_what_no_waveform_could_be_made_from() {
        assert!(WaveformOptions::default().validate().is_ok());
        for bad in [
            WaveformOptions {
                frames_per_peak: 4,
                ..WaveformOptions::default()
            },
            WaveformOptions {
                frames_per_peak: MAX_FRAMES_PER_PEAK + 1,
                ..WaveformOptions::default()
            },
            WaveformOptions {
                levels: 0,
                ..WaveformOptions::default()
            },
            WaveformOptions {
                levels: MAX_WAVEFORM_LEVELS + 1,
                ..WaveformOptions::default()
            },
            WaveformOptions {
                decimation: 1,
                ..WaveformOptions::default()
            },
            WaveformOptions {
                decimation: MAX_WAVEFORM_DECIMATION + 1,
                ..WaveformOptions::default()
            },
        ] {
            let err = bad.validate().expect_err("these options are unusable");
            assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
        }
    }

    #[test]
    fn levels_step_by_the_decimation_and_options_fingerprint_them() {
        let options = WaveformOptions::default();
        assert_eq!(options.frames_per_peak_at(0), 256);
        assert_eq!(options.frames_per_peak_at(1), 1_024);
        assert_eq!(options.frames_per_peak_at(5), 256 * 4_u64.pow(5));
        // Past the last level is clamped to the last, never overflowed.
        assert_eq!(
            options.frames_per_peak_at(99),
            options.frames_per_peak_at(5)
        );
        assert_eq!(options.fingerprint(), "p256d4l6");
        assert_ne!(
            options.fingerprint(),
            WaveformOptions {
                levels: 4,
                ..options
            }
            .fingerprint()
        );
    }

    #[test]
    fn peaks_quantise_and_round_trip_through_their_bytes() {
        let peak = Peak::SILENCE.with_sample(1.0).with_sample(-0.5);
        assert_eq!(peak.max, i16::MAX);
        assert_eq!(peak.min, -16_384);
        assert_eq!(Peak::from_le_bytes(peak.to_le_bytes()), peak);
        // Out-of-range samples are clamped, never wrapped.
        let loud = Peak::SILENCE.with_sample(4.0).with_sample(-4.0);
        assert_eq!(loud.max, i16::MAX);
        assert_eq!(loud.min, -i16::MAX);
        assert!((peak.max_f32() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn peaks_merge_to_the_widest_excursion() {
        let a = Peak { min: -10, max: 20 };
        let b = Peak { min: -30, max: 5 };
        assert_eq!(a.merged(b), Peak { min: -30, max: 20 });
        assert_eq!(a.merged(Peak::SILENCE), Peak { min: -10, max: 20 });
    }

    #[test]
    fn the_builder_buckets_by_absolute_frame_position() {
        let mut builder = PeakBuilder::new(2, 4);
        builder.push_block(0, &ramp(8));
        let (peaks, frames) = builder.finish();
        assert_eq!(frames, 8);
        assert_eq!(peaks.len(), 4, "two buckets of two channels");
        // The left channel climbs, so the second bucket is louder.
        assert!(peaks[2].max > peaks[0].max);
        // The right channel is the negation of the left.
        assert_eq!(peaks[1].min, -peaks[0].max);
        assert_eq!(peaks[1].max, 0);
    }

    #[test]
    fn the_builder_leaves_silence_where_the_stream_has_a_gap() {
        let mut builder = PeakBuilder::new(1, 4);
        builder.push_block(0, &[1.0, 1.0, 1.0, 1.0]);
        // Nothing between frame 4 and frame 12.
        builder.push_block(12, &[-1.0, -1.0, -1.0, -1.0]);
        let (peaks, frames) = builder.finish();
        assert_eq!(frames, 16);
        assert_eq!(peaks.len(), 4);
        assert_eq!(peaks[0].max, i16::MAX);
        assert_eq!(peaks[1], Peak::SILENCE, "the gap is silent");
        assert_eq!(peaks[2], Peak::SILENCE, "the gap is silent");
        assert_eq!(peaks[3].min, -i16::MAX);
    }

    #[test]
    fn a_partial_last_bucket_is_kept_rather_than_dropped() {
        let mut builder = PeakBuilder::new(1, 4);
        builder.push_block(0, &[0.5, 0.5, 0.5, 0.5, 0.25]);
        let (peaks, frames) = builder.finish();
        assert_eq!(frames, 5);
        assert_eq!(peaks.len(), 2);
        assert!(peaks[1].max > 0, "the tail of the file is still drawn");
    }

    #[test]
    fn decimation_merges_whole_groups_and_keeps_the_remainder() {
        let peaks = vec![
            Peak { min: -1, max: 1 },
            Peak { min: -2, max: 2 },
            Peak { min: -30, max: 3 },
            Peak { min: -4, max: 40 },
            Peak { min: -5, max: 5 },
            Peak { min: -6, max: 6 },
        ];
        // One channel, groups of two: three coarser peaks.
        let coarse = decimate(&peaks, 1, 2);
        assert_eq!(
            coarse,
            vec![
                Peak { min: -2, max: 2 },
                Peak { min: -30, max: 40 },
                Peak { min: -6, max: 6 },
            ]
        );
        // Two channels, groups of two: the channels never mix.
        let stereo = decimate(&peaks, 2, 2);
        assert_eq!(
            stereo,
            vec![
                Peak { min: -30, max: 3 },
                Peak { min: -4, max: 40 },
                Peak { min: -5, max: 5 },
                Peak { min: -6, max: 6 },
            ]
        );
        // A group that is not full still produces a peak.
        assert_eq!(decimate(&peaks, 1, 4).len(), 2);
        assert!(decimate(&[], 2, 4).is_empty());
    }

    #[test]
    fn decimation_never_loses_the_loudest_sample() {
        let mut builder = PeakBuilder::new(1, 2);
        builder.push_block(0, &ramp(64).iter().step_by(2).copied().collect::<Vec<_>>());
        let (base, _) = builder.finish();
        let loudest = base.iter().map(|peak| peak.max).max().expect("peaks");
        let mut coarse = base;
        for _ in 0..3 {
            coarse = decimate(&coarse, 1, 4);
        }
        assert_eq!(
            coarse.iter().map(|peak| peak.max).max(),
            Some(loudest),
            "a coarser level still shows the loudest sample under it"
        );
    }

    #[test]
    fn level_choice_takes_the_coarsest_level_a_pixel_can_still_show() {
        let waveform = waveform_of(&[256, 1_024, 4_096], 2, 48_000, 480_000);
        assert_eq!(waveform.level_for(4_096).map(|l| l.index), Some(2));
        assert_eq!(waveform.level_for(5_000).map(|l| l.index), Some(2));
        assert_eq!(waveform.level_for(1_024).map(|l| l.index), Some(1));
        assert_eq!(waveform.level_for(1_023).map(|l| l.index), Some(0));
        // Zoomed in past the base level there is nothing finer to show.
        assert_eq!(waveform.level_for(1).map(|l| l.index), Some(0));
        assert_eq!(waveform.level_for(0).map(|l| l.index), Some(0));
    }

    #[test]
    fn duration_is_exact_at_the_source_sample_rate() {
        let waveform = waveform_of(&[256], 2, 48_000, 480_000);
        let duration = waveform.duration();
        assert_eq!(duration.value(), 480_000);
        assert_eq!(duration.rate().numerator(), 48_000);
        assert_eq!(
            duration.rescaled_to(crate::probe::NANOSECONDS).value(),
            10_000_000_000
        );
    }

    #[test]
    fn peak_files_round_trip_through_the_sidecar_directory() {
        let dir = std::env::temp_dir().join(format!("sub-wave-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("level.peaks");
        let peaks = vec![
            Peak { min: -1, max: 2 },
            Peak {
                min: i16::MIN,
                max: i16::MAX,
            },
        ];
        write_peak_file(&path, &peaks).expect("the peaks must be writable");
        assert_eq!(read_peak_file(&path, 2).expect("read back"), peaks);
        // A wrong length is a corrupt file, not a shorter waveform.
        let err = read_peak_file(&path, 3).expect_err("a wrong length must be rejected");
        assert_eq!(err.code, codes::WAVEFORM_FAILED);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A waveform describing levels of the given resolutions, with no files
    /// behind it: enough for the pure lookups.
    fn waveform_of(
        frames_per_peak: &[u64],
        channels: u16,
        sample_rate: u32,
        frames: u64,
    ) -> Waveform {
        let options = WaveformOptions::default();
        let hash = ContentHash::from_bytes([7; 32]);
        Waveform {
            cache_dir: PathBuf::from("."),
            source_hash: hash,
            options,
            sample_rate,
            channels,
            frames,
            levels: frames_per_peak
                .iter()
                .enumerate()
                .map(|(index, step)| WaveformLevel {
                    index: u32::try_from(index).expect("a small level index"),
                    frames_per_peak: *step,
                    peaks: frames / step,
                    file: Waveform::level_file_name(
                        hash,
                        options,
                        u32::try_from(index).expect("a small level index"),
                    ),
                })
                .collect(),
        }
    }
}
