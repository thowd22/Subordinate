//! Importing media: hashing, probing and thumbnails, all as jobs.
//!
//! Import is the one place the media bin touches the filesystem, and none of
//! that work may happen on the UI thread: a single long file can take seconds
//! to probe. Every imported file therefore becomes one job on the
//! [`JobService`](sub_core::JobService) — hash the bytes, probe the streams,
//! build the [`MediaItem`] — and, once that lands, a second job for the
//! thumbnail strip (docs/PLAN.md §5.2).
//!
//! [`ImportQueue`] is what the panel holds. It is pumped once a frame with
//! [`ImportQueue::poll`], never blocks, and hands back finished items for the
//! host to apply through [`ImportMedia`](sub_edit::commands::ImportMedia).
//! Nothing here mutates a project: an import that has not been applied as a
//! command has not happened.
//!
//! Media paths are project-relative by model rule, so a file outside the
//! project folder is refused with `model.invalid_path` rather than being
//! stored as an absolute path. Copying footage into the project folder before
//! importing is the user's job, and the failure says so.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sub_core::{JobContext, JobHandle, JobService, Priority, SubError, SubResult};
use sub_media::{
    MediaInfo, ProbeOptions, ThumbnailJob, ThumbnailOptions, probe_with, spawn_thumbnail_job,
};
use sub_model::media::{AudioStream, StreamInfo, VideoStream};
use sub_model::{BinId, ContentHash, MediaItem, MediaPath};
use sub_time::Rational;

/// The job kind every import reports itself as.
pub const IMPORT_JOB_KIND: &str = "import";

/// The rate recorded for a video stream whose container declares none.
///
/// A rate-less stream is a still image or a malformed file; the model has no
/// way to say "unknown", so one picture per second stands in and the bin shows
/// it as such rather than pretending to a rate the file never claimed.
pub const UNKNOWN_FRAME_RATE: Rational = Rational::ONE;

/// How an import runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportOptions {
    /// The probe budget and scan depth.
    pub probe: ProbeOptions,
    /// The strip generated for each imported item.
    pub thumbnails: ThumbnailOptions,
    /// The priority both jobs run at. Import is what the user is watching, so
    /// it defaults to [`Priority::Interactive`]; the strip that follows runs
    /// at [`Priority::Normal`], because the bin can draw a row without it.
    pub priority: Priority,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            probe: ProbeOptions::default(),
            thumbnails: ThumbnailOptions::default(),
            priority: Priority::Interactive,
        }
    }
}

/// What one finished import produced.
#[derive(Debug, Clone)]
pub enum ImportOutcome {
    /// The file was hashed and probed; the host applies this item as an
    /// `ImportMedia` command.
    Ready {
        /// The item, with its hash and probe result already filled in.
        item: Box<MediaItem>,
        /// The bin the user dropped it on, or the root bin when absent.
        bin: Option<BinId>,
    },
    /// The file could not be imported and nothing was added to the project.
    Failed {
        /// The file that was asked for.
        path: PathBuf,
        /// Why it was refused: `model.invalid_path` for a file outside the
        /// project folder, `core.io` for one that cannot be read, or whatever
        /// the probe reported.
        error: SubError,
    },
}

/// The model's view of what the probe found.
///
/// Only what the project file keeps survives the conversion: codecs, rotation
/// and seekability are diagnostics, not project state. Width and height stay
/// as coded, so a rotated file reports the frames it actually holds and the
/// viewer applies the rotation when it draws them.
#[must_use]
pub fn stream_info(info: &MediaInfo) -> StreamInfo {
    StreamInfo {
        duration: info.duration,
        video: info
            .video
            .iter()
            .map(|stream| VideoStream {
                width: stream.width,
                height: stream.height,
                frame_rate: stream.frame_rate.unwrap_or(UNKNOWN_FRAME_RATE),
                sample_aspect: stream.sample_aspect,
                color: stream.color,
            })
            .collect(),
        audio: info
            .audio
            .iter()
            .map(|stream| AudioStream {
                channels: stream.channels,
                sample_rate: stream.sample_rate,
            })
            .collect(),
    }
}

/// Builds the media item for `file`, which must sit inside `project_dir`.
///
/// # Errors
///
/// Returns `model.invalid_path` when `file` is not inside the project folder.
pub fn imported_item(
    project_dir: &Path,
    file: &Path,
    hash: ContentHash,
    info: &MediaInfo,
) -> SubResult<MediaItem> {
    let path = MediaPath::relative_to(project_dir, file)?;
    let mut item = MediaItem::new(path);
    item.hash = Some(hash);
    item.info = Some(stream_info(info));
    Ok(item)
}

/// One import running on a [`JobService`].
#[derive(Debug, Clone)]
pub struct ImportJob {
    handle: JobHandle,
    item: Arc<Mutex<Option<MediaItem>>>,
}

impl ImportJob {
    /// The underlying job handle: state, priority, cancellation.
    #[must_use]
    pub fn handle(&self) -> &JobHandle {
        &self.handle
    }

    /// Asks the job to stop; it does so between its steps.
    pub fn cancel(&self) {
        self.handle.cancel();
    }

    /// Blocks until the job finishes and returns the item it built.
    ///
    /// # Errors
    ///
    /// Returns the job's error, or `core.cancelled` when it was cancelled.
    pub fn wait(&self) -> SubResult<MediaItem> {
        self.handle.wait().into_result()?;
        self.take().ok_or_else(|| {
            SubError::new(
                sub_core::codes::INTERNAL,
                "a completed import job left no media item",
            )
        })
    }

    /// The item a finished job built, taken out of the job.
    fn take(&self) -> Option<MediaItem> {
        self.item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

/// Queues the import of `file` on `jobs`.
///
/// Returns at once. The worker expresses the file relative to `project_dir`,
/// hashes its bytes, probes its streams and builds the item; the caller
/// applies that item as a command.
pub fn spawn_import_job(
    jobs: &JobService,
    project_dir: &Path,
    file: &Path,
    options: ImportOptions,
) -> ImportJob {
    let project_dir = project_dir.to_path_buf();
    let file = file.to_path_buf();
    let item = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&item);
    let handle = jobs.submit(
        IMPORT_JOB_KIND,
        options.priority,
        move |ctx: &JobContext| {
            // The cheapest check first: a file outside the project folder is
            // refused before a byte of it is read.
            let path = MediaPath::relative_to(&project_dir, &file)?;
            ctx.check()?;
            let hash = ContentHash::of_file(&file)?;
            ctx.progress(1, 3);
            ctx.check()?;
            let info = probe_with(&file, options.probe)?;
            ctx.progress(2, 3);

            let mut built = MediaItem::new(path);
            built.hash = Some(hash);
            built.info = Some(stream_info(&info));
            *slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(built);
            ctx.progress(3, 3);
            Ok(())
        },
    );
    ImportJob { handle, item }
}

/// One import in flight, and where its item is to be filed.
#[derive(Debug)]
struct Pending {
    job: ImportJob,
    source: PathBuf,
    bin: Option<BinId>,
}

/// The imports and thumbnail strips a media bin has in flight.
///
/// Held by the host, pumped once a frame. Dropping it leaves the jobs running;
/// [`ImportQueue::cancel_all`] stops them.
#[derive(Debug)]
pub struct ImportQueue {
    project_dir: PathBuf,
    cache_dir: PathBuf,
    options: ImportOptions,
    imports: Vec<Pending>,
    thumbnails: Vec<ThumbnailJob>,
}

impl ImportQueue {
    /// A queue importing into `project_dir`, writing strips into `cache_dir`
    /// (the project's `.sub.d` sidecar folder).
    #[must_use]
    pub fn new(project_dir: impl Into<PathBuf>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
            cache_dir: cache_dir.into(),
            options: ImportOptions::default(),
            imports: Vec::new(),
            thumbnails: Vec::new(),
        }
    }

    /// The same queue, running with `options`.
    #[must_use]
    pub fn with_options(mut self, options: ImportOptions) -> Self {
        self.options = options;
        self
    }

    /// The project folder every imported path is expressed relative to.
    #[must_use]
    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    /// Queues one job per path, filing what they produce in `bin`.
    pub fn submit(&mut self, jobs: &JobService, paths: &[PathBuf], bin: Option<BinId>) {
        for path in paths {
            let job = spawn_import_job(jobs, &self.project_dir, path, self.options);
            self.imports.push(Pending {
                job,
                source: path.clone(),
                bin,
            });
        }
    }

    /// Collects the imports that have finished since the last call, and queues
    /// a thumbnail strip for each one that succeeded.
    ///
    /// Finished strips are dropped: their pictures are files on disk, named
    /// after the source's content hash, so the bin finds them without holding
    /// the job.
    pub fn poll(&mut self, jobs: &JobService) -> Vec<ImportOutcome> {
        let mut finished = Vec::new();
        let mut still_running = Vec::with_capacity(self.imports.len());
        for pending in std::mem::take(&mut self.imports) {
            if pending.job.handle().is_finished() {
                finished.push(pending);
            } else {
                still_running.push(pending);
            }
        }
        self.imports = still_running;
        self.thumbnails.retain(|job| !job.handle().is_finished());

        finished
            .into_iter()
            .map(|pending| match pending.job.wait() {
                Ok(item) => {
                    self.queue_thumbnails(jobs, &pending.source, &item);
                    ImportOutcome::Ready {
                        item: Box::new(item),
                        bin: pending.bin,
                    }
                }
                Err(error) => ImportOutcome::Failed {
                    path: pending.source,
                    error,
                },
            })
            .collect()
    }

    /// Queues the strip for a freshly imported item, when it has pictures to
    /// make one from.
    fn queue_thumbnails(&mut self, jobs: &JobService, source: &Path, item: &MediaItem) {
        let has_video = item.info.as_ref().is_some_and(StreamInfo::has_video);
        let has_duration = item
            .info
            .as_ref()
            .and_then(|info| info.duration)
            .is_some_and(|duration| !duration.is_zero());
        if !(has_video && has_duration) {
            return;
        }
        self.thumbnails.push(spawn_thumbnail_job(
            jobs,
            source,
            &self.cache_dir,
            self.options.thumbnails,
            Priority::Normal,
        ));
    }

    /// How many imports are still running.
    #[must_use]
    pub fn importing(&self) -> usize {
        self.imports.len()
    }

    /// How many thumbnail strips are still being generated.
    #[must_use]
    pub fn thumbnailing(&self) -> usize {
        self.thumbnails.len()
    }

    /// True when nothing is in flight.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.imports.is_empty() && self.thumbnails.is_empty()
    }

    /// Asks every job in flight to stop.
    pub fn cancel_all(&self) {
        for pending in &self.imports {
            pending.job.cancel();
        }
        for job in &self.thumbnails {
            job.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ImportOutcome, ImportQueue, UNKNOWN_FRAME_RATE, stream_info};
    use std::path::PathBuf;
    use sub_core::JobService;
    use sub_media::probe::{AudioStreamInfo, FrameTiming, MediaInfo, Rotation, VideoStreamInfo};
    use sub_model::ColorTags;
    use sub_time::{Rational, RationalTime};

    /// A probe result for a one-second 1080p file with stereo sound.
    fn probed(frame_rate: Option<Rational>) -> MediaInfo {
        MediaInfo {
            container: "video/quicktime".to_owned(),
            container_format: None,
            duration: Some(RationalTime::new(24, Rational::FPS_24)),
            seekable: true,
            video: vec![VideoStreamInfo {
                codec: "video/x-h264".to_owned(),
                codec_description: "H.264".to_owned(),
                width: 1920,
                height: 1080,
                frame_rate,
                sample_aspect: Rational::ONE,
                rotation: Rotation::None,
                mirrored: false,
                color: ColorTags::REC709,
                interlaced: false,
                timing: FrameTiming::Constant,
            }],
            audio: vec![AudioStreamInfo {
                codec: "audio/mpeg".to_owned(),
                codec_description: "AAC".to_owned(),
                channels: 2,
                sample_rate: 48_000,
                language: None,
            }],
        }
    }

    #[test]
    fn the_probe_result_becomes_what_the_project_file_keeps() {
        let info = stream_info(&probed(Some(Rational::FPS_23_976)));
        assert_eq!(info.duration, Some(RationalTime::new(24, Rational::FPS_24)));
        assert_eq!(info.video.len(), 1);
        assert_eq!(info.video[0].frame_rate, Rational::FPS_23_976);
        assert_eq!(info.video[0].width, 1920);
        assert_eq!(info.audio[0].channels, 2);
        assert_eq!(info.audio[0].sample_rate, 48_000);
    }

    #[test]
    fn a_stream_with_no_declared_rate_gets_the_stand_in_rate() {
        let info = stream_info(&probed(None));
        assert_eq!(info.video[0].frame_rate, UNKNOWN_FRAME_RATE);
    }

    #[test]
    fn a_file_outside_the_project_folder_fails_the_import_and_adds_nothing() {
        // The failure happens before a byte is read, so this needs neither a
        // real file nor GStreamer.
        let jobs = JobService::new(1);
        let mut queue = ImportQueue::new("/projects/cut", "/projects/cut.sub.d");
        queue.submit(&jobs, &[PathBuf::from("/elsewhere/take.mov")], None);
        assert_eq!(queue.importing(), 1);

        jobs.wait_idle();
        let outcomes = queue.poll(&jobs);
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            ImportOutcome::Failed { path, error } => {
                assert_eq!(path, &PathBuf::from("/elsewhere/take.mov"));
                assert_eq!(error.code, sub_model::codes::INVALID_PATH);
            }
            ImportOutcome::Ready { .. } => panic!("the import should have been refused"),
        }
        assert!(queue.is_idle(), "a refused import queues no thumbnails");
    }

    #[test]
    fn a_missing_file_inside_the_project_folder_fails_on_its_hash() {
        let jobs = JobService::new(1);
        let dir = std::env::temp_dir().join("sub-ui-import-missing");
        let mut queue = ImportQueue::new(&dir, dir.join("cut.sub.d"));
        queue.submit(&jobs, &[dir.join("gone.mov")], None);

        jobs.wait_idle();
        let outcomes = queue.poll(&jobs);
        assert!(matches!(outcomes[0], ImportOutcome::Failed { .. }));
        assert_eq!(queue.importing(), 0);
    }
}
