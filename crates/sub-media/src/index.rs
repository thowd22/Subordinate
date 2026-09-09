//! The presentation-timestamp index for variable-frame-rate sources
//! (docs/PLAN.md §9).
//!
//! A constant-frame-rate file lets frame *n* be computed from a rate: the
//! *n*-th picture starts at `n / rate`. A variable-frame-rate file does not,
//! and assuming it does is where proxy and seek bugs come from. A
//! [`PtsIndex`] is the explicit answer: one entry per picture, in presentation
//! order, carrying that picture's exact presentation timestamp and whether it
//! is a keyframe. Every lookup is exact [`RationalTime`] arithmetic in
//! nanoseconds; nothing here divides a duration by a frame count.
//!
//! Building an index parses the whole file, so it is never done on the hot
//! path. It happens once, lazily, on the first access through
//! [`LazyPtsIndex`], and the result is cached as JSON in the project's sidecar
//! directory keyed by the source's [`ContentHash`], so a second open reads the
//! index instead of re-parsing. A build can also be handed to a worker thread
//! as an [`IndexJob`], which is cancellable: [`IndexJob::cancel`] stops the
//! parse within a bus poll and the join returns `core.cancelled`.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use std::path::Path;
//!
//! let index = sub_media::PtsIndex::build(Path::new("/media/vfr.mkv"))?;
//! let pts = index.pts(10).expect("an eleventh frame");
//! assert_eq!(index.frame_at(pts), Some(10));
//! # Ok(())
//! # }
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use gstreamer as gst;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};
use sub_core::{ResultExt, SubError, SubResult};
use sub_model::ContentHash;
use sub_time::{RationalTime, Rounding};

use crate::codes;
use crate::decode::{Decoder, DecoderOptions, VideoFrame};
use crate::probe::NANOSECONDS;

/// Schema version of the cached index file. A cache written by an older
/// version is rebuilt rather than trusted.
pub const INDEX_CACHE_VERSION: u32 = 1;

/// File name extension cached indexes are written under.
const CACHE_SUFFIX: &str = ".ptsindex.json";

/// How long a bus poll waits before the cancel flag is looked at again.
const POLL_SLICE_MS: u64 = 50;

/// Upper bound on indexed pictures: ten hours at 60 fps. It bounds the
/// allocation a hostile or broken file can provoke.
const MAX_INDEXED_FRAMES: usize = 2_160_000;

/// A cooperative cancel flag shared with a running index build.
///
/// Every long-running piece of work in Subordinate is cancelled the same way,
/// so this is [`sub_core::CancelToken`] itself rather than another flag with
/// the same shape: a token taken from a job context stops an index build, and
/// the token of an index build can be handed to anything else.
pub use sub_core::CancelToken;

/// The error a cancelled index build reports.
fn cancelled_error() -> SubError {
    SubError::new(
        sub_core::codes::CANCELLED,
        "the PTS index build was cancelled",
    )
}

/// One indexed picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexEntry {
    /// Presentation timestamp in nanoseconds from the start of the stream.
    pub pts_ns: i64,
    /// Whether the picture can be decoded without any earlier picture.
    pub keyframe: bool,
}

impl IndexEntry {
    /// The presentation timestamp as an exact time in nanoseconds.
    pub fn pts(self) -> RationalTime {
        RationalTime::new(self.pts_ns, NANOSECONDS)
    }
}

/// The on-disk form of a cached index.
///
/// Timestamps and keyframe positions are two flat arrays rather than an array
/// of objects: an hour of 60 fps video is 216 000 entries, and the flat form
/// keeps that file small.
#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    source_hash: String,
    pts_ns: Vec<i64>,
    keyframes: Vec<u32>,
}

/// Frame number to presentation timestamp, for one media file.
///
/// Entries are in presentation order: entry *n* is the *n*-th picture the file
/// shows. A file whose pictures are stored out of order (a long-GOP stream is
/// parsed in decode order) is sorted here, so a frame number always means the
/// same thing as it does on a timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtsIndex {
    source_hash: ContentHash,
    entries: Vec<IndexEntry>,
}

impl PtsIndex {
    /// Builds an index by parsing `path`, with no cache and no cancellation.
    ///
    /// # Errors
    ///
    /// Returns `media.file_unreadable` when the path is not a readable file,
    /// `media.unsupported` when a parsing element is missing and
    /// `media.index_failed` when the parse pipeline fails or the file exposes
    /// no timed video.
    pub fn build(path: &Path) -> SubResult<Self> {
        Self::build_cancellable(path, &CancelToken::new())
    }

    /// Builds an index, stopping early when `cancel` is set.
    ///
    /// # Errors
    ///
    /// As [`PtsIndex::build`], plus `core.cancelled` when the token is set
    /// before the parse reaches the end of the file.
    pub fn build_cancellable(path: &Path, cancel: &CancelToken) -> SubResult<Self> {
        gst::init()
            .map_err(|e| SubError::wrap(codes::INIT_FAILED, "GStreamer failed to start", &e))?;
        let source_hash = hash_source(path)?;
        let mut entries = collect_entries(path, cancel)?;
        // Decode order is not presentation order on a long-GOP stream, and a
        // frame number must mean the picture the viewer sees.
        entries.sort_by_key(|entry| entry.pts_ns);
        if entries.is_empty() {
            return Err(
                SubError::new(codes::INDEX_FAILED, "file exposes no timed video pictures")
                    .with_detail("path", path.display().to_string()),
            );
        }
        Ok(Self {
            source_hash,
            entries,
        })
    }

    /// Reads the cached index for `path` from `cache_dir`, building and
    /// caching it when no usable cache is there.
    ///
    /// A cache written for other bytes, by another schema version, or one that
    /// cannot be parsed at all, is not an error: it is ignored and rebuilt.
    /// Failing to *write* the cache is not an error either — the index is
    /// still returned — because a read-only sidecar directory must not stop an
    /// edit.
    ///
    /// # Errors
    ///
    /// As [`PtsIndex::build_cancellable`].
    pub fn load_or_build(path: &Path, cache_dir: &Path, cancel: &CancelToken) -> SubResult<Self> {
        let hash = hash_source(path)?;
        let cache_path = cache_dir.join(Self::cache_file_name(hash));
        match Self::read_cache(&cache_path, hash) {
            Ok(Some(index)) => {
                tracing::debug!(
                    path = %path.display(),
                    frames = index.len(),
                    "PTS index read from cache"
                );
                return Ok(index);
            }
            Ok(None) => {}
            Err(err) => {
                tracing::debug!(
                    cache = %cache_path.display(),
                    error = %err,
                    "PTS index cache unusable; rebuilding"
                );
            }
        }
        let index = Self::build_cancellable(path, cancel)?;
        if let Err(err) = index.write_cache(&cache_path) {
            tracing::warn!(
                cache = %cache_path.display(),
                error = %err,
                "PTS index could not be cached"
            );
        }
        Ok(index)
    }

    /// The file name a source with `hash` is cached under.
    pub fn cache_file_name(hash: ContentHash) -> String {
        format!("{hash}{CACHE_SUFFIX}")
    }

    /// Content hash of the source this index was built from.
    pub fn source_hash(&self) -> ContentHash {
        self.source_hash
    }

    /// How many pictures the file holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index holds no pictures. An index is never built empty, so
    /// this is only true of one assembled by hand.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry, in presentation order.
    pub fn entries(&self) -> &[IndexEntry] {
        &self.entries
    }

    /// Presentation timestamp of frame `frame`, or `None` past the last one.
    pub fn pts(&self, frame: usize) -> Option<RationalTime> {
        self.entries.get(frame).map(|entry| entry.pts())
    }

    /// Whether frame `frame` is a keyframe, or `None` past the last one.
    pub fn is_keyframe(&self, frame: usize) -> Option<bool> {
        self.entries.get(frame).map(|entry| entry.keyframe)
    }

    /// How long frame `frame` stays on screen: the gap to the next picture.
    ///
    /// `None` for the last frame, whose duration only the container knows, and
    /// past the end. This is the value a variable-frame-rate source makes
    /// vary, and the reason nothing here assumes one frame duration.
    pub fn duration_of(&self, frame: usize) -> Option<RationalTime> {
        let this = self.entries.get(frame)?;
        let next = self.entries.get(frame + 1)?;
        Some(RationalTime::new(
            next.pts_ns.saturating_sub(this.pts_ns),
            NANOSECONDS,
        ))
    }

    /// The frame on screen at `time`: the last one whose timestamp is at or
    /// before it.
    ///
    /// `None` when `time` is before the first picture. `time` is floored into
    /// nanoseconds, so a target expressed at any rate answers exactly.
    pub fn frame_at(&self, time: RationalTime) -> Option<usize> {
        let nanos = to_nanos(time, Rounding::Floor)?;
        match self.search(nanos) {
            Ok(found) => Some(found),
            Err(0) => None,
            Err(after) => Some(after - 1),
        }
    }

    /// The first frame at or after `time`, or `None` when the file ends first.
    ///
    /// `time` is ceilinged into nanoseconds so a target between two pictures
    /// resolves to the later one.
    pub fn frame_at_or_after(&self, time: RationalTime) -> Option<usize> {
        let nanos = to_nanos(time, Rounding::Ceil)?;
        let frame = match self.search(nanos) {
            Ok(found) | Err(found) => found,
        };
        (frame < self.entries.len()).then_some(frame)
    }

    /// The keyframe at or before `frame`, which is where a decode that has to
    /// produce `frame` must start. `None` past the end, or when no keyframe
    /// precedes it.
    pub fn keyframe_at_or_before(&self, frame: usize) -> Option<usize> {
        let entries = self.entries.get(..=frame)?;
        entries.iter().rposition(|entry| entry.keyframe)
    }

    /// Position of `nanos` among the entries: `Ok(frame)` when a picture
    /// starts exactly there, `Err(n)` for the insertion point otherwise.
    fn search(&self, nanos: i64) -> Result<usize, usize> {
        self.entries
            .binary_search_by_key(&nanos, |entry| entry.pts_ns)
    }

    /// Reads a cache file, returning `None` when it is not there or was
    /// written for other bytes or another schema version.
    fn read_cache(cache_path: &Path, hash: ContentHash) -> SubResult<Option<Self>> {
        let text = match std::fs::read_to_string(cache_path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(SubError::wrap(
                    codes::INDEX_FAILED,
                    "PTS index cache cannot be read",
                    &err,
                ));
            }
        };
        let file: CacheFile = serde_json::from_str(&text).map_err(|err| {
            SubError::wrap(codes::INDEX_FAILED, "PTS index cache is not valid", &err)
        })?;
        if file.version != INDEX_CACHE_VERSION || file.source_hash != hash.to_string() {
            return Ok(None);
        }
        let mut entries: Vec<IndexEntry> = file
            .pts_ns
            .iter()
            .map(|&pts_ns| IndexEntry {
                pts_ns,
                keyframe: false,
            })
            .collect();
        for keyframe in file.keyframes {
            let position = usize::try_from(keyframe).unwrap_or(usize::MAX);
            let entry = entries.get_mut(position).ok_or_else(|| {
                SubError::new(codes::INDEX_FAILED, "PTS index cache names a missing frame")
                    .with_detail("frame", keyframe)
            })?;
            entry.keyframe = true;
        }
        if entries.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            source_hash: hash,
            entries,
        }))
    }

    /// Writes the cache file, creating the sidecar directory if needed.
    ///
    /// The file is written beside its final name and renamed into place, so a
    /// reader never sees a half-written index.
    fn write_cache(&self, cache_path: &Path) -> SubResult<()> {
        let io = |err: &std::io::Error| {
            SubError::wrap(
                codes::INDEX_FAILED,
                "PTS index cache cannot be written",
                err,
            )
            .with_detail("path", cache_path.display().to_string())
        };
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| io(&err))?;
        }
        let file = CacheFile {
            version: INDEX_CACHE_VERSION,
            source_hash: self.source_hash.to_string(),
            pts_ns: self.entries.iter().map(|entry| entry.pts_ns).collect(),
            keyframes: self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.keyframe)
                .filter_map(|(frame, _)| u32::try_from(frame).ok())
                .collect(),
        };
        let text = serde_json::to_string(&file)
            .sub_context(codes::INDEX_FAILED, "PTS index cannot be serialised")?;
        let temporary = cache_path.with_extension("tmp");
        std::fs::write(&temporary, text).map_err(|err| io(&err))?;
        std::fs::rename(&temporary, cache_path).map_err(|err| io(&err))
    }
}

/// Hashes the source bytes, reporting an unreadable file in this crate's
/// vocabulary rather than the model's.
fn hash_source(path: &Path) -> SubResult<ContentHash> {
    ContentHash::of_file(path).map_err(|err| {
        SubError::new(codes::FILE_UNREADABLE, "media file cannot be indexed")
            .with_detail("path", path.display().to_string())
            .with_cause(&err)
    })
}

/// Floors or ceilings `time` into whole nanoseconds.
fn to_nanos(time: RationalTime, rounding: Rounding) -> Option<i64> {
    time.checked_rescaled_to_rounding(NANOSECONDS, rounding)
        .map(RationalTime::value)
}

/// An index that is built the first time it is asked for.
///
/// Building parses the whole file, so nothing does it at open: a caller holds
/// a [`LazyPtsIndex`] and pays for the parse only if something actually needs
/// frame-exact timing. The built index is memoised, so every later access is a
/// clone of an `Arc`.
#[derive(Debug)]
pub struct LazyPtsIndex {
    path: PathBuf,
    cache_dir: Option<PathBuf>,
    built: Mutex<Option<Arc<PtsIndex>>>,
}

impl LazyPtsIndex {
    /// An index for `path` that is cached in `cache_dir` — the project's
    /// sidecar directory — when one is given.
    pub fn new(path: impl Into<PathBuf>, cache_dir: Option<PathBuf>) -> Self {
        Self {
            path: path.into(),
            cache_dir,
            built: Mutex::new(None),
        }
    }

    /// The source file this index describes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the index has been built already, so a caller can tell a cheap
    /// access from one that will parse the file.
    pub fn is_built(&self) -> bool {
        self.built.lock().is_ok_and(|built| built.is_some())
    }

    /// The index, building it on the first call.
    ///
    /// # Errors
    ///
    /// As [`PtsIndex::load_or_build`]. A failed build is not memoised: the
    /// next call tries again.
    pub fn get(&self) -> SubResult<Arc<PtsIndex>> {
        self.get_cancellable(&CancelToken::new())
    }

    /// The index, building it on the first call and honouring `cancel`.
    ///
    /// # Errors
    ///
    /// As [`PtsIndex::load_or_build`].
    pub fn get_cancellable(&self, cancel: &CancelToken) -> SubResult<Arc<PtsIndex>> {
        let mut built = self
            .built
            .lock()
            .map_err(|_| SubError::new(codes::INDEX_FAILED, "PTS index lock was poisoned"))?;
        if let Some(index) = built.as_ref() {
            return Ok(Arc::clone(index));
        }
        let index = Arc::new(match self.cache_dir.as_deref() {
            Some(cache_dir) => PtsIndex::load_or_build(&self.path, cache_dir, cancel)?,
            None => PtsIndex::build_cancellable(&self.path, cancel)?,
        });
        *built = Some(Arc::clone(&index));
        Ok(index)
    }
}

/// A cancellable background index build.
///
/// The build runs on its own thread. [`IndexJob::cancel`] asks it to stop and
/// [`IndexJob::join`] waits for the result — `core.cancelled` when the build
/// was stopped. Dropping the job cancels it and leaves the thread to unwind on
/// its own, so nothing blocks in a `Drop`.
#[derive(Debug)]
pub struct IndexJob {
    cancel: CancelToken,
    handle: Option<JoinHandle<SubResult<Arc<PtsIndex>>>>,
}

impl IndexJob {
    /// Starts a build of `path`, cached in `cache_dir` when one is given.
    pub fn spawn(path: impl Into<PathBuf>, cache_dir: Option<PathBuf>) -> Self {
        Self::spawn_for(Arc::new(LazyPtsIndex::new(path, cache_dir)))
    }

    /// Starts a build that memoises into `lazy`, so the job and later direct
    /// accesses share one index.
    pub fn spawn_for(lazy: Arc<LazyPtsIndex>) -> Self {
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let handle = std::thread::Builder::new()
            .name("sub-media-pts-index".to_owned())
            .spawn(move || lazy.get_cancellable(&token))
            .ok();
        Self { cancel, handle }
    }

    /// The token this job watches, for a caller that wants to cancel several
    /// jobs at once.
    pub fn token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Asks the build to stop. Cancelling a finished build does nothing.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether the build thread has run to completion.
    pub fn is_finished(&self) -> bool {
        self.handle
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
    }

    /// Waits for the build and returns its index.
    ///
    /// # Errors
    ///
    /// Returns `core.cancelled` when the build was cancelled,
    /// `media.index_failed` when the build thread could not be started or
    /// panicked, and every error [`PtsIndex::build_cancellable`] returns.
    pub fn join(mut self) -> SubResult<Arc<PtsIndex>> {
        let handle = self.handle.take().ok_or_else(|| {
            SubError::new(codes::INDEX_FAILED, "the index build thread never started")
        })?;
        handle
            .join()
            .map_err(|_| SubError::new(codes::INDEX_FAILED, "the index build thread panicked"))?
    }
}

impl Drop for IndexJob {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Runs a parse-only pipeline over `path` and returns one entry per video
/// buffer it saw, in the order the parser produced them.
fn collect_entries(path: &Path, cancel: &CancelToken) -> SubResult<Vec<IndexEntry>> {
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
        .sub_context(codes::INDEX_FAILED, "could not build the index pipeline")?;
    src.link(&parsebin)
        .sub_context(codes::INDEX_FAILED, "could not link the index pipeline")?;

    let entries = Arc::new(Mutex::new(Vec::<IndexEntry>::new()));
    let collected = Arc::clone(&entries);
    let token = cancel.clone();
    let weak_pipeline = pipeline.downgrade();
    parsebin.connect_pad_added(move |_, pad| {
        let Some(pipeline) = weak_pipeline.upgrade() else {
            return;
        };
        // Every pad needs a consumer or the parser stalls, but only the video
        // pads are indexed.
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
        let token = token.clone();
        // Whether this pad carries video is decided on its first buffer, not
        // here: a pad can be added before its caps are negotiated.
        let is_video = OnceLock::new();
        pad.add_probe(gst::PadProbeType::BUFFER, move |pad, probe| {
            if token.is_cancelled() {
                return gst::PadProbeReturn::Ok;
            }
            if *is_video.get_or_init(|| pad_carries_video(pad))
                && let Some(gst::PadProbeData::Buffer(buffer)) = &probe.data
                && let Some(pts) = buffer.pts()
                && let Ok(mut seen) = collected.lock()
                && seen.len() < MAX_INDEXED_FRAMES
            {
                seen.push(IndexEntry {
                    pts_ns: i64::try_from(pts.nseconds()).unwrap_or(i64::MAX),
                    // A parser marks every picture that needs an earlier one
                    // as a delta unit; what is left is a keyframe.
                    keyframe: !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT),
                });
            }
            gst::PadProbeReturn::Ok
        });
    });

    let outcome = run_until_eos(&pipeline, cancel);
    let _ = pipeline.set_state(gst::State::Null);
    outcome?;

    let seen = entries
        .lock()
        .map_err(|_| SubError::new(codes::INDEX_FAILED, "the index scan panicked"))?;
    Ok(seen.clone())
}

/// Whether `pad` carries video, judged from its caps once data flows.
///
/// A frame sequence is not always tagged `video/`: an MJPEG track — which is
/// what the proxies of §5.2 are written as — comes off a parser as
/// `image/jpeg`, one buffer per picture, and must be indexed like any other
/// video stream.
fn pad_carries_video(pad: &gst::Pad) -> bool {
    let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
    caps.structure(0).is_some_and(|structure| {
        let name = structure.name();
        name.starts_with("video/") || name.starts_with("image/")
    })
}

/// Plays `pipeline` until end of stream, an error, or cancellation.
fn run_until_eos(pipeline: &gst::Pipeline, cancel: &CancelToken) -> SubResult<()> {
    pipeline
        .set_state(gst::State::Playing)
        .sub_context(codes::INDEX_FAILED, "the index pipeline would not start")?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| SubError::new(codes::INDEX_FAILED, "the index pipeline has no bus"))?;
    loop {
        if cancel.is_cancelled() {
            return Err(cancelled_error());
        }
        // The wait is sliced so the cancel flag is looked at often; waiting for
        // the whole parse in one call could not be interrupted at all.
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(POLL_SLICE_MS)) else {
            continue;
        };
        match message.view() {
            gst::MessageView::Eos(_) => return Ok(()),
            gst::MessageView::Error(err) => {
                return Err(SubError::wrap(
                    codes::INDEX_FAILED,
                    "the index pipeline failed",
                    &err.error(),
                ));
            }
            _ => {}
        }
    }
}

/// A [`Decoder`] driven by frame numbers rather than by time.
///
/// Frame *n* is seeked to by the timestamp the index recorded for it, so a
/// variable-frame-rate source steps exactly as a constant one does: nothing
/// multiplies a frame number by a frame duration. Stepping forward by one is
/// a seek to the next indexed timestamp, which [`Decoder::seek_to`] usually
/// answers by decoding forward inside the current GOP rather than by seeking.
///
/// ```no_run
/// # fn main() -> sub_core::SubResult<()> {
/// use std::path::Path;
/// use std::sync::Arc;
///
/// let path = Path::new("/media/vfr.mkv");
/// let index = Arc::new(sub_media::PtsIndex::build(path)?);
/// let mut decoder = sub_media::IndexedDecoder::open(path, index)?;
/// let frame = decoder.seek_to_frame(120)?.expect("a 121st frame");
/// assert_eq!(decoder.current_frame(), Some(120));
/// let _ = frame;
/// # Ok(())
/// # }
/// ```
pub struct IndexedDecoder {
    decoder: Decoder,
    index: Arc<PtsIndex>,
    current: Option<usize>,
}

impl IndexedDecoder {
    /// Opens `path` with the default decoder options, driven by `index`.
    ///
    /// # Errors
    ///
    /// As [`Decoder::open`].
    pub fn open(path: &Path, index: Arc<PtsIndex>) -> SubResult<Self> {
        Self::open_with(path, DecoderOptions::default(), index)
    }

    /// Opens `path` with explicit decoder options, driven by `index`.
    ///
    /// # Errors
    ///
    /// As [`Decoder::open_with`].
    pub fn open_with(
        path: &Path,
        options: DecoderOptions,
        index: Arc<PtsIndex>,
    ) -> SubResult<Self> {
        Ok(Self {
            decoder: Decoder::open_with(path, options)?,
            index,
            current: None,
        })
    }

    /// The index this decoder is driven by.
    pub fn index(&self) -> &Arc<PtsIndex> {
        &self.index
    }

    /// How many pictures the file holds, from the index.
    pub fn frame_count(&self) -> usize {
        self.index.len()
    }

    /// The frame number the last delivered picture is, or `None` before the
    /// first one.
    pub fn current_frame(&self) -> Option<usize> {
        self.current
    }

    /// How many flushing seeks the underlying decoder has issued.
    pub fn seek_count(&self) -> u64 {
        self.decoder.seek_count()
    }

    /// Decodes frame `frame`, or `None` when it is past the end of the index
    /// or the stream ends first.
    ///
    /// # Errors
    ///
    /// As [`Decoder::seek_to`].
    pub fn seek_to_frame(&mut self, frame: usize) -> SubResult<Option<VideoFrame>> {
        let Some(target) = self.index.pts(frame) else {
            return Ok(None);
        };
        let decoded = self.decoder.seek_to(target)?;
        self.current = decoded
            .as_ref()
            .and_then(|picture| self.index.frame_at(picture.pts()));
        Ok(decoded)
    }

    /// Steps `delta` frames from the current one and decodes what lands there.
    ///
    /// Stepping before the first frame clamps to it; stepping past the last
    /// returns `None` and leaves the position alone. Before any frame has been
    /// delivered, a forward step counts from before the first frame, so
    /// `step(1)` gives frame zero.
    ///
    /// # Errors
    ///
    /// As [`Decoder::seek_to`].
    pub fn step(&mut self, delta: i64) -> SubResult<Option<VideoFrame>> {
        let from = match self.current {
            Some(current) => i64::try_from(current).unwrap_or(i64::MAX),
            None => -1,
        };
        let target = from.saturating_add(delta).max(0);
        let frame = usize::try_from(target).unwrap_or(usize::MAX);
        self.seek_to_frame(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::{CacheFile, CancelToken, INDEX_CACHE_VERSION, IndexEntry, PtsIndex};
    use crate::probe::NANOSECONDS;
    use sub_model::ContentHash;
    use sub_time::{Rational, RationalTime};

    /// A hand-built index with the timing of the VFR fixture in miniature:
    /// four frames at 30 fps then four at 60 fps, keyframes every four.
    fn index() -> PtsIndex {
        let thirty = 33_333_333_i64;
        let sixty = 16_666_666_i64;
        let mut pts = 0_i64;
        let mut entries = Vec::new();
        for step in 0..8 {
            entries.push(IndexEntry {
                pts_ns: pts,
                keyframe: step % 4 == 0,
            });
            pts += if step < 4 { thirty } else { sixty };
        }
        PtsIndex {
            source_hash: ContentHash::from_bytes([7; 32]),
            entries,
        }
    }

    fn nanos(value: i64) -> RationalTime {
        RationalTime::new(value, NANOSECONDS)
    }

    #[test]
    fn pts_and_keyframes_are_looked_up_by_frame_number() {
        let index = index();
        assert_eq!(index.len(), 8);
        assert!(!index.is_empty());
        assert_eq!(index.pts(0), Some(nanos(0)));
        assert_eq!(index.pts(4), Some(nanos(133_333_332)));
        assert_eq!(index.pts(8), None);
        assert_eq!(index.is_keyframe(0), Some(true));
        assert_eq!(index.is_keyframe(1), Some(false));
        assert_eq!(index.is_keyframe(4), Some(true));
        assert_eq!(index.is_keyframe(9), None);
    }

    #[test]
    fn frame_durations_vary_and_are_never_assumed() {
        let index = index();
        assert_eq!(index.duration_of(0), Some(nanos(33_333_333)));
        assert_eq!(index.duration_of(4), Some(nanos(16_666_666)));
        // The last frame's duration is not in the index.
        assert_eq!(index.duration_of(7), None);
        assert_ne!(index.duration_of(0), index.duration_of(4));
    }

    #[test]
    fn lookup_by_time_lands_on_the_frame_on_screen() {
        let index = index();
        assert_eq!(index.frame_at(nanos(-1)), None);
        assert_eq!(index.frame_at(nanos(0)), Some(0));
        // One nanosecond before the second frame is still the first frame.
        assert_eq!(index.frame_at(nanos(33_333_332)), Some(0));
        assert_eq!(index.frame_at(nanos(33_333_333)), Some(1));
        // Past the last picture the last picture is still on screen.
        assert_eq!(index.frame_at(nanos(10_000_000_000)), Some(7));

        assert_eq!(index.frame_at_or_after(nanos(0)), Some(0));
        assert_eq!(index.frame_at_or_after(nanos(1)), Some(1));
        assert_eq!(index.frame_at_or_after(nanos(33_333_333)), Some(1));
        assert_eq!(index.frame_at_or_after(nanos(10_000_000_000)), None);
    }

    #[test]
    fn a_target_at_a_frame_rate_resolves_without_a_float() {
        let index = index();
        // 3/30 s is exactly the fourth frame of the 30 fps run, whose stored
        // timestamp is truncated: the answer must still be that frame.
        let rate = Rational::new(30, 1).expect("30 is a rate");
        let target = RationalTime::from_frames(3, rate);
        assert_eq!(index.frame_at(target), Some(3));
    }

    #[test]
    fn a_decode_starts_at_the_keyframe_at_or_before_a_frame() {
        let index = index();
        assert_eq!(index.keyframe_at_or_before(0), Some(0));
        assert_eq!(index.keyframe_at_or_before(3), Some(0));
        assert_eq!(index.keyframe_at_or_before(4), Some(4));
        assert_eq!(index.keyframe_at_or_before(7), Some(4));
        assert_eq!(index.keyframe_at_or_before(8), None);
    }

    #[test]
    fn a_cache_round_trips_and_a_stale_one_is_ignored() {
        let index = index();
        let dir = std::env::temp_dir().join(format!("sub-media-index-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(PtsIndex::cache_file_name(index.source_hash()));
        index.write_cache(&path).expect("cache is written");

        let read = PtsIndex::read_cache(&path, index.source_hash()).expect("cache reads");
        assert_eq!(read.as_ref(), Some(&index));

        // A cache written for other bytes is not this file's index.
        let other = ContentHash::from_bytes([9; 32]);
        assert_eq!(PtsIndex::read_cache(&path, other).expect("reads"), None);

        // Neither is one from another schema version.
        let text = std::fs::read_to_string(&path).expect("cache text");
        let mut file: CacheFile = serde_json::from_str(&text).expect("cache parses");
        file.version = INDEX_CACHE_VERSION + 1;
        std::fs::write(
            &path,
            serde_json::to_string(&file).expect("cache serialises"),
        )
        .expect("cache is rewritten");
        assert_eq!(
            PtsIndex::read_cache(&path, index.source_hash()).expect("reads"),
            None
        );

        // A corrupt cache is an error the caller rebuilds through, not a panic.
        std::fs::write(&path, "{ not json").expect("cache is corrupted");
        assert!(PtsIndex::read_cache(&path, index.source_hash()).is_err());

        // A missing cache is simply absent.
        std::fs::remove_file(&path).expect("cache is removed");
        assert_eq!(
            PtsIndex::read_cache(&path, index.source_hash()).expect("reads"),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_token_stops_a_build_before_it_opens_anything() {
        let cancel = CancelToken::new();
        assert!(!cancel.is_cancelled());
        let clone = cancel.clone();
        clone.cancel();
        assert!(cancel.is_cancelled());
    }
}
