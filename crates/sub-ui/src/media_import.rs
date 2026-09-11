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
    MediaInfo, ProbeOptions, ThumbnailJob, ThumbnailOptions, WaveformJob, WaveformOptions,
    probe_with, spawn_thumbnail_job, spawn_waveform_job,
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
    /// The peak pyramid generated for each imported item that has sound.
    pub waveforms: WaveformOptions,
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
            waveforms: WaveformOptions::default(),
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

/// One batch of imports: everything one Import gesture asked for.
///
/// A gesture is one undo step, so the host waits for the whole batch before it
/// applies anything. The identifier is what lets it tell two overlapping
/// gestures apart — a drop from the desktop while a dialog selection is still
/// probing is two batches, and two entries in the undo stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImportBatch(u64);

impl ImportBatch {
    /// The batch's number, in the order the queue handed them out.
    #[must_use]
    pub const fn index(self) -> u64 {
        self.0
    }
}

/// Everything one Import gesture produced, once all of it has finished.
///
/// The host turns the [`ImportOutcome::Ready`] entries into one history group
/// and shows the [`ImportOutcome::Failed`] ones in the bin.
#[derive(Debug, Clone)]
pub struct FinishedImport {
    /// The gesture these outcomes belong to.
    pub batch: ImportBatch,
    /// The bin every file in the gesture was filed in.
    pub bin: Option<BinId>,
    /// One outcome per file asked for, in the order they were asked for.
    pub outcomes: Vec<ImportOutcome>,
}

/// One import in flight, and where its item is to be filed.
#[derive(Debug)]
struct Pending {
    job: ImportJob,
    source: PathBuf,
    bin: Option<BinId>,
    batch: ImportBatch,
    /// Where in its batch this file was asked for, so the outcomes come back
    /// in the order the user chose the files rather than the order the jobs
    /// happened to finish.
    slot: usize,
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
    /// The batches asked for and not yet collected, each with the bin its
    /// files are filed in. A batch is only collected once nothing of it is
    /// still probing, which is what makes one gesture one undo step.
    open: Vec<(ImportBatch, Option<BinId>)>,
    /// The outcomes of a batch whose other files are still probing, kept until
    /// the batch is whole.
    held: Vec<(ImportBatch, usize, ImportOutcome)>,
    thumbnails: Vec<ThumbnailJob>,
    waveforms: Vec<WaveformJob>,
    next_batch: u64,
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
            open: Vec::new(),
            held: Vec::new(),
            thumbnails: Vec::new(),
            waveforms: Vec::new(),
            next_batch: 0,
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
    ///
    /// The paths are one gesture, so they are one batch: nothing is applied
    /// until every one of them has finished, and the whole batch then becomes
    /// a single undo step. An empty `paths` still takes a batch number and
    /// comes straight back out of the next [`ImportQueue::poll`] with no
    /// outcomes, so a caller never has to special-case it.
    pub fn submit(
        &mut self,
        jobs: &JobService,
        paths: &[PathBuf],
        bin: Option<BinId>,
    ) -> ImportBatch {
        let batch = ImportBatch(self.next_batch);
        self.next_batch += 1;
        self.open.push((batch, bin));
        for (slot, path) in paths.iter().enumerate() {
            let job = spawn_import_job(jobs, &self.project_dir, path, self.options);
            self.imports.push(Pending {
                job,
                source: path.clone(),
                bin,
                batch,
                slot,
            });
        }
        batch
    }

    /// Collects the batches that have finished since the last call, and queues
    /// a thumbnail strip and a waveform for each file that succeeded.
    ///
    /// A batch appears exactly once, and only when every file in it has
    /// finished, because the host applies it as one undo step. Finished strips
    /// and waveforms are dropped: both write files on disk named after the
    /// source's content hash, so the bin and the timeline find them without
    /// holding the job.
    pub fn poll(&mut self, jobs: &JobService) -> Vec<FinishedImport> {
        let mut still_running = Vec::with_capacity(self.imports.len());
        for pending in std::mem::take(&mut self.imports) {
            if !pending.job.handle().is_finished() {
                still_running.push(pending);
                continue;
            }
            let outcome = match pending.job.wait() {
                Ok(item) => {
                    self.queue_followups(jobs, &pending.source, &item);
                    ImportOutcome::Ready {
                        item: Box::new(item),
                        bin: pending.bin,
                    }
                }
                Err(error) => ImportOutcome::Failed {
                    path: pending.source,
                    error,
                },
            };
            self.held.push((pending.batch, pending.slot, outcome));
        }
        self.imports = still_running;
        self.thumbnails.retain(|job| !job.handle().is_finished());
        self.waveforms.retain(|job| !job.handle().is_finished());

        // A batch is whole once none of its files is still probing. That is
        // the whole of the batching rule: the host applies what comes back
        // here as one history group, so half a gesture is never applied.
        let running = &self.imports;
        let (whole, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.open)
            .into_iter()
            .partition(|(batch, _)| !running.iter().any(|pending| pending.batch == *batch));
        self.open = waiting;

        let mut finished: Vec<FinishedImport> = Vec::with_capacity(whole.len());
        for (batch, bin) in whole {
            let mut entries: Vec<(usize, ImportOutcome)> = Vec::new();
            self.held.retain(|(held, slot, outcome)| {
                if *held == batch {
                    entries.push((*slot, outcome.clone()));
                    false
                } else {
                    true
                }
            });
            entries.sort_by_key(|(slot, _)| *slot);
            finished.push(FinishedImport {
                batch,
                bin,
                outcomes: entries.into_iter().map(|(_, outcome)| outcome).collect(),
            });
        }
        finished.sort_by_key(|done| done.batch);
        finished
    }

    /// Queues the strip and the peaks for a freshly imported item.
    ///
    /// Both are only worth asking for when the file has the stream they
    /// summarise: a sound-only file gets no strip, and a silent one gets no
    /// waveform.
    fn queue_followups(&mut self, jobs: &JobService, source: &Path, item: &MediaItem) {
        let info = item.info.as_ref();
        let has_duration = info
            .and_then(|info| info.duration)
            .is_some_and(|duration| !duration.is_zero());
        if !has_duration {
            return;
        }
        if info.is_some_and(StreamInfo::has_video) {
            self.thumbnails.push(spawn_thumbnail_job(
                jobs,
                source,
                &self.cache_dir,
                self.options.thumbnails,
                Priority::Normal,
            ));
        }
        if info.is_some_and(|info| !info.audio.is_empty()) {
            self.waveforms.push(spawn_waveform_job(
                jobs,
                source,
                &self.cache_dir,
                self.options.waveforms,
                Priority::Normal,
            ));
        }
    }

    /// The files still being hashed and probed, in the order they were asked
    /// for.
    ///
    /// This is what the bin draws as its pending state: a row per file that
    /// has been asked for and has not yet become a media item.
    #[must_use]
    pub fn pending_paths(&self) -> Vec<&Path> {
        self.imports
            .iter()
            .map(|pending| pending.source.as_path())
            .collect()
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

    /// How many waveforms are still being generated.
    #[must_use]
    pub fn waveforming(&self) -> usize {
        self.waveforms.len()
    }

    /// True when nothing is in flight.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.imports.is_empty() && self.thumbnails.is_empty() && self.waveforms.is_empty()
    }

    /// Asks every job in flight to stop.
    pub fn cancel_all(&self) {
        for pending in &self.imports {
            pending.job.cancel();
        }
        for job in &self.thumbnails {
            job.cancel();
        }
        for job in &self.waveforms {
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
        assert_eq!(
            queue.pending_paths(),
            [std::path::Path::new("/elsewhere/take.mov")],
            "the bin has a pending row to draw while the job runs"
        );

        jobs.wait_idle();
        let mut finished = queue.poll(&jobs);
        assert_eq!(finished.len(), 1, "one gesture, one batch");
        let batch = finished.remove(0);
        assert_eq!(batch.outcomes.len(), 1);
        match &batch.outcomes[0] {
            ImportOutcome::Failed { path, error } => {
                assert_eq!(path, &PathBuf::from("/elsewhere/take.mov"));
                assert_eq!(error.code, sub_model::codes::INVALID_PATH);
            }
            ImportOutcome::Ready { .. } => panic!("the import should have been refused"),
        }
        assert!(
            queue.is_idle(),
            "a refused import queues no thumbnails and no waveform"
        );
    }

    #[test]
    fn a_missing_file_inside_the_project_folder_fails_on_its_hash() {
        let jobs = JobService::new(1);
        let dir = std::env::temp_dir().join("sub-ui-import-missing");
        let mut queue = ImportQueue::new(&dir, dir.join("cut.sub.d"));
        queue.submit(&jobs, &[dir.join("gone.mov")], None);

        jobs.wait_idle();
        let finished = queue.poll(&jobs);
        assert!(matches!(
            finished[0].outcomes[0],
            ImportOutcome::Failed { .. }
        ));
        assert_eq!(queue.importing(), 0);
    }
}
