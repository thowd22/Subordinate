//! The export job: a render with progress, an ETA, cancellation and an error
//! that names the element that failed (TASK-61, docs/PLAN.md §5.5).
//!
//! [`ExportPipeline`] knows how to push frames; this is the loop around it
//! that a person or an agent watches. It reports an [`ExportEvent`] as it
//! goes — an [`ExportProgress`] snapshot carrying frames done, the estimated
//! wall-clock time left and the [`EncoderStats`] of the running encoders — and
//! it checks a [`CancelToken`] between frames. A cancelled or failed export
//! deletes the part-written file: a muxer that never saw end of stream leaves
//! a broken file wearing a real name, and nobody should be handed that.
//!
//! Every derived number is integer arithmetic. Rates and percentages are
//! carried in thousandths so a progress event is exactly reproducible and no
//! float ever reaches a report, a log line or an agent.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use std::path::Path;
//! use sub_core::jobs::CancelToken;
//! use sub_export::{
//!     Container, EncoderPreferences, ExportElements, ExportEvent, ExportJob, ExportSettings,
//!     SolidFrames,
//! };
//! use sub_time::Rational;
//!
//! let settings = ExportSettings::new(1920, 1080, Rational::FPS_24, Container::Mp4)
//!     .with_audio_codec(None);
//! let elements = ExportElements::resolve(&settings, &EncoderPreferences::new())?;
//! let cancel = CancelToken::new();
//! let job = ExportJob::new(Path::new("out.mp4"), &settings, &elements)
//!     .with_total_frames(240)
//!     .with_cancel(cancel.clone());
//!
//! let mut frames = SolidFrames::new(settings.frame_bytes(), 240);
//! let report = job.run(&mut frames, None, &mut |event| {
//!     if let ExportEvent::Progress(progress) = event {
//!         println!("{}%", progress.percent_milli.unwrap_or(0) / 1000);
//!     }
//! })?;
//! println!("wrote {}", report.path.display());
//! # Ok(())
//! # }
//! ```

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sub_core::jobs::{CancelToken, JobContext, JobHandle, JobService, Priority};
use sub_core::{SubError, SubResult};
use sub_time::RationalTime;

use crate::pipeline::{
    AudioFrameSource, ExportElements, ExportPipeline, ExportReport, ExportSettings,
    VideoFrameSource, remove_partial_file,
};

/// The job kind every export event carries on a [`JobService`].
pub const EXPORT_JOB_KIND: &str = "export";

/// How often progress is reported when the caller does not say.
pub const DEFAULT_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Nanoseconds in one second.
const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// Thousandths, the unit rates and percentages are carried in.
const MILLI: u128 = 1_000;

/// What the encoders and the muxer have done so far.
///
/// These are the numbers a person judges an export by: which elements are
/// running, how much of each stream has gone through them, and how big the
/// file has grown. The file size comes from the file itself, so a muxer that
/// holds its header back reports zero for a moment — it is a statistic, never
/// a completion test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncoderStats {
    /// The video encoder element that is running.
    pub video_encoder: String,
    /// The audio encoder element, when the export has audio.
    pub audio_encoder: Option<String>,
    /// The muxer element writing the file.
    pub muxer: String,
    /// Video frames pushed into the encoder so far.
    pub video_frames: u64,
    /// Audio frames pushed into the encoder so far, silence padding included.
    pub audio_frames: u64,
    /// Bytes on disk so far.
    pub bytes_written: u64,
    /// The media time written so far, at the sequence frame rate.
    pub encoded: RationalTime,
}

impl EncoderStats {
    /// The average bitrate of what has been written, in bits per second of
    /// media, or `None` before any media time exists.
    ///
    /// Computed from the exact rational duration, so the answer does not
    /// wobble with the frame rate the way a float-seconds division would.
    pub fn bits_per_second(&self) -> Option<u64> {
        let (seconds_num, seconds_den) = self.encoded.as_seconds_fraction();
        if seconds_num <= 0 || seconds_den <= 0 {
            return None;
        }
        let bits = i128::from(self.bytes_written).checked_mul(8)?;
        let rate = bits.checked_mul(seconds_den)? / seconds_num;
        u64::try_from(rate).ok()
    }
}

/// One progress snapshot of a running export.
///
/// `frames_total` is `0` when the caller did not say how long the export is,
/// and then there is no percentage and no ETA to give.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportProgress {
    /// Video frames written so far.
    pub frames_done: u64,
    /// Video frames the export expects in total, or `0` when unknown.
    pub frames_total: u64,
    /// Wall-clock time since the pipeline started.
    pub elapsed: Duration,
    /// Estimated wall-clock time left, or `None` while it cannot be known.
    pub eta: Option<Duration>,
    /// Encode rate in thousandths of a frame per second.
    pub frames_per_second_milli: u64,
    /// How far along the export is, in thousandths of a percent, or `None`
    /// when the total is unknown.
    pub percent_milli: Option<u64>,
    /// What the encoders and the muxer have done.
    pub stats: EncoderStats,
}

impl ExportProgress {
    /// How far along the export is, in whole percent, or `None` when the total
    /// is unknown.
    pub fn percent(&self) -> Option<u64> {
        self.percent_milli.map(|milli| milli / 1_000)
    }

    /// Frames still to write, or `None` when the total is unknown.
    pub fn frames_remaining(&self) -> Option<u64> {
        if self.frames_total == 0 {
            return None;
        }
        Some(self.frames_total.saturating_sub(self.frames_done))
    }
}

/// Something that happened to a running export.
///
/// The sequence is always one `Started`, any number of `Progress`, then
/// exactly one of `Finished`, `Cancelled` or `Failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportEvent {
    /// The pipeline is up and the first frame is about to go in.
    Started {
        /// The file being written.
        path: PathBuf,
        /// Video frames expected, or `0` when unknown.
        frames_total: u64,
        /// The elements that will run.
        stats: EncoderStats,
    },
    /// A progress snapshot.
    Progress(ExportProgress),
    /// The export wrote its file.
    Finished(ExportReport),
    /// The export was cancelled between frames.
    Cancelled {
        /// Frames written before the cancel was seen.
        frames_done: u64,
        /// Whether the part-written file was removed.
        file_removed: bool,
    },
    /// The export failed.
    Failed {
        /// What went wrong; a pipeline failure names the element that posted
        /// it in both its message and its `element` detail.
        error: SubError,
        /// Frames written before the failure.
        frames_done: u64,
        /// Whether the part-written file was removed.
        file_removed: bool,
    },
}

/// An export configured to run: what to write, how to encode it, how long it
/// is, and how to stop it.
#[derive(Debug, Clone)]
pub struct ExportJob {
    path: PathBuf,
    settings: ExportSettings,
    elements: ExportElements,
    frames_total: u64,
    cancel: CancelToken,
    progress_interval: Duration,
}

impl ExportJob {
    /// A job writing `path` with `settings` through `elements`.
    ///
    /// Without [`ExportJob::with_total_frames`] the job runs until the frame
    /// source runs dry and reports no percentage and no ETA.
    pub fn new(path: &Path, settings: &ExportSettings, elements: &ExportElements) -> Self {
        Self {
            path: path.to_owned(),
            settings: settings.clone(),
            elements: elements.clone(),
            frames_total: 0,
            cancel: CancelToken::new(),
            progress_interval: DEFAULT_PROGRESS_INTERVAL,
        }
    }

    /// The same job told how many video frames it will write, which is what
    /// makes a percentage and an ETA possible.
    #[must_use]
    pub fn with_total_frames(mut self, frames: u64) -> Self {
        self.frames_total = frames;
        self
    }

    /// The same job watching `cancel`.
    #[must_use]
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// The same job reporting progress no more often than `interval`.
    ///
    /// [`Duration::ZERO`] reports every frame; the first and last frame are
    /// always reported whatever the interval.
    #[must_use]
    pub fn with_progress_interval(mut self, interval: Duration) -> Self {
        self.progress_interval = interval;
        self
    }

    /// The file the job writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The settings the job encodes with.
    pub fn settings(&self) -> &ExportSettings {
        &self.settings
    }

    /// Video frames the job expects to write, or `0` when unknown.
    pub fn frames_total(&self) -> u64 {
        self.frames_total
    }

    /// The token that stops the job.
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Asks the job to stop at the next frame boundary.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Runs the export to completion, reporting every [`ExportEvent`] to
    /// `events`.
    ///
    /// Cancellation is checked before every frame: a cancelled export stops
    /// the pipeline, deletes the part-written file and returns
    /// [`sub_core::codes::CANCELLED`]. A failure does the same and returns the
    /// pipeline's error, which names the element that posted it.
    ///
    /// # Errors
    ///
    /// The errors of [`ExportPipeline::new`], [`ExportPipeline::push_video_frame`]
    /// and [`ExportPipeline::finish`], whatever the sources raise, and
    /// [`sub_core::codes::CANCELLED`] when the token is set.
    pub fn run(
        &self,
        video: &mut dyn VideoFrameSource,
        audio: Option<&mut dyn AudioFrameSource>,
        events: &mut dyn FnMut(&ExportEvent),
    ) -> SubResult<ExportReport> {
        let started = Instant::now();
        let mut pipeline = match ExportPipeline::new(&self.path, &self.settings, &self.elements) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                // Nothing ran, so nothing is half-written; the file is still
                // removed because a failed start can leave an empty one.
                let file_removed = remove_partial_file(&self.path);
                events(&ExportEvent::Failed {
                    error: error.clone(),
                    frames_done: 0,
                    file_removed,
                });
                return Err(error);
            }
        };
        events(&ExportEvent::Started {
            path: self.path.clone(),
            frames_total: self.frames_total,
            stats: self.stats(&pipeline),
        });

        if let Err(error) = self.pump(&mut pipeline, video, audio, events, started) {
            let frames_done = pipeline.video_frames();
            let file_removed = pipeline.abort();
            events(&ended(error.clone(), frames_done, file_removed));
            return Err(error);
        }

        let frames_done = pipeline.video_frames();
        match pipeline.finish() {
            Ok(report) => {
                events(&ExportEvent::Finished(report.clone()));
                Ok(report)
            }
            Err(error) => {
                let file_removed = remove_partial_file(&self.path);
                events(&ended(error.clone(), frames_done, file_removed));
                Err(error)
            }
        }
    }

    /// Pushes every frame the sources have, checking the cancel token and
    /// reporting progress as it goes.
    fn pump(
        &self,
        pipeline: &mut ExportPipeline,
        video: &mut dyn VideoFrameSource,
        mut audio: Option<&mut dyn AudioFrameSource>,
        events: &mut dyn FnMut(&ExportEvent),
        started: Instant,
    ) -> SubResult<()> {
        let channels = usize::from(self.settings.channels.max(1));
        let mut block: Vec<f32> = Vec::new();
        let mut last_report: Option<Instant> = None;
        let mut reported = u64::MAX;
        loop {
            if self.cancel.is_cancelled() {
                return Err(CancelToken::cancelled_error("the export"));
            }
            let Some(pixels) = video.next_frame()? else {
                break;
            };
            let wanted = pipeline.audio_frames_for_next_video_frame();
            pipeline.push_video_frame(pixels)?;
            if pipeline.has_audio() {
                let samples = usize::try_from(wanted).unwrap_or(usize::MAX) * channels;
                block.clear();
                block.resize(samples, 0.0);
                if let Some(source) = audio.as_deref_mut() {
                    let filled = source.read(&mut block, self.settings.channels)?;
                    // Silence past the end of the mix keeps the streams the
                    // same length, so the file ends on one timestamp.
                    for sample in &mut block[filled * channels..] {
                        *sample = 0.0;
                    }
                }
                pipeline.push_audio(&block)?;
            }

            let done = pipeline.video_frames();
            let due = last_report.is_none_or(|at| at.elapsed() >= self.progress_interval);
            if due || done == self.frames_total {
                events(&ExportEvent::Progress(self.progress(pipeline, started)));
                last_report = Some(Instant::now());
                reported = done;
            }
        }
        // The last frame always gets a snapshot, so a watcher's final reading
        // is the export's real frame count and not whatever the interval left.
        let done = pipeline.video_frames();
        if reported != done {
            events(&ExportEvent::Progress(self.progress(pipeline, started)));
        }
        Ok(())
    }

    /// What the encoders have done so far.
    fn stats(&self, pipeline: &ExportPipeline) -> EncoderStats {
        EncoderStats {
            video_encoder: self.elements.video_encoder.clone(),
            audio_encoder: self.elements.audio_encoder.clone(),
            muxer: self.settings.container.muxer().to_owned(),
            video_frames: pipeline.video_frames(),
            audio_frames: pipeline.audio_frames(),
            bytes_written: pipeline.bytes_written(),
            encoded: self.settings.duration(pipeline.video_frames()),
        }
    }

    /// One progress snapshot, taken now.
    fn progress(&self, pipeline: &ExportPipeline, started: Instant) -> ExportProgress {
        let elapsed = started.elapsed();
        let done = pipeline.video_frames();
        ExportProgress {
            frames_done: done,
            frames_total: self.frames_total,
            elapsed,
            eta: estimate_remaining(elapsed, done, self.frames_total),
            frames_per_second_milli: encode_rate_milli(elapsed, done),
            percent_milli: percent_milli(done, self.frames_total),
            stats: self.stats(pipeline),
        }
    }
}

/// The terminal event for `error`: cancellation reads differently from a
/// failure even though both stop the export the same way.
fn ended(error: SubError, frames_done: u64, file_removed: bool) -> ExportEvent {
    if error.code == sub_core::codes::CANCELLED {
        ExportEvent::Cancelled {
            frames_done,
            file_removed,
        }
    } else {
        ExportEvent::Failed {
            error,
            frames_done,
            file_removed,
        }
    }
}

/// The wall-clock time `total - done` frames will take at the rate the export
/// has managed so far, or `None` while there is nothing to extrapolate from.
///
/// The estimate is the elapsed time scaled by the work left, in whole
/// nanoseconds: no float, and no division by a frame count of zero.
fn estimate_remaining(elapsed: Duration, done: u64, total: u64) -> Option<Duration> {
    if total == 0 {
        return None;
    }
    if done >= total {
        return Some(Duration::ZERO);
    }
    if done == 0 {
        return None;
    }
    let remaining = u128::from(total - done);
    let nanos = elapsed.as_nanos().checked_mul(remaining)? / u128::from(done);
    Some(Duration::from_nanos(
        u64::try_from(nanos).unwrap_or(u64::MAX),
    ))
}

/// Frames per second so far, in thousandths.
fn encode_rate_milli(elapsed: Duration, done: u64) -> u64 {
    let nanos = elapsed.as_nanos();
    if nanos == 0 || done == 0 {
        return 0;
    }
    let rate = u128::from(done) * MILLI * NANOS_PER_SECOND / nanos;
    u64::try_from(rate).unwrap_or(u64::MAX)
}

/// Progress in thousandths of a percent, or `None` when the total is unknown.
fn percent_milli(done: u64, total: u64) -> Option<u64> {
    if total == 0 {
        return None;
    }
    let percent = u128::from(done) * 100 * MILLI / u128::from(total);
    Some(u64::try_from(percent.min(100 * MILLI)).unwrap_or(u64::MAX))
}

/// A running export on a [`JobService`]: its handle, its events and its report.
#[derive(Debug)]
pub struct ExportJobHandle {
    handle: JobHandle,
    events: Receiver<ExportEvent>,
    report: Arc<Mutex<Option<ExportReport>>>,
}

impl ExportJobHandle {
    /// The underlying job handle: state, priority, cancellation.
    pub fn handle(&self) -> &JobHandle {
        &self.handle
    }

    /// Asks the export to stop; it does so at the next frame boundary and
    /// deletes the part-written file.
    pub fn cancel(&self) {
        self.handle.cancel();
    }

    /// The export's own event stream, carrying the ETA and the encoder stats
    /// that a [`sub_core::JobEvent`] has no room for.
    pub fn events(&self) -> &Receiver<ExportEvent> {
        &self.events
    }

    /// Blocks until the export finishes and reports what it wrote.
    ///
    /// # Errors
    ///
    /// The job's error, or [`sub_core::codes::CANCELLED`] when it was
    /// cancelled.
    pub fn wait(&self) -> SubResult<ExportReport> {
        self.handle.wait().into_result()?;
        let report = self
            .report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        report.ok_or_else(|| {
            SubError::new(
                sub_core::codes::INTERNAL,
                "a completed export job left no report",
            )
        })
    }
}

/// Queues `job` on `jobs` and returns at once.
///
/// The export runs on a worker thread. Frame counts reach
/// [`JobService::subscribe`] subscribers as
/// [`JobEvent::Progress`](sub_core::JobEvent::Progress), and the full
/// [`ExportEvent`] stream — ETA and encoder stats included — reaches
/// [`ExportJobHandle::events`]. An export is what the person is waiting for,
/// so [`Priority::Normal`] or higher is the usual choice.
pub fn spawn_export_job(
    jobs: &JobService,
    job: ExportJob,
    video: Box<dyn VideoFrameSource + Send>,
    audio: Option<Box<dyn AudioFrameSource + Send>>,
    priority: Priority,
) -> ExportJobHandle {
    let (sender, events) = channel();
    let report = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&report);
    let handle = jobs.submit(EXPORT_JOB_KIND, priority, move |ctx: &JobContext| {
        let job = job.with_cancel(ctx.cancel_token());
        let mut video = video;
        let mut audio = audio;
        let audio = audio
            .as_deref_mut()
            .map(|source| source as &mut dyn AudioFrameSource);
        let produced = job.run(&mut *video, audio, &mut |event| {
            if let ExportEvent::Progress(progress) = event {
                ctx.progress(progress.frames_done, progress.frames_total);
            }
            // A watcher that dropped its receiver is not a reason to stop.
            let _ = sender.send(event.clone());
        })?;
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(produced);
        Ok(())
    });
    ExportJobHandle {
        handle,
        events,
        report,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sub_time::{Rational, RationalTime};

    use super::{
        EncoderStats, ExportProgress, encode_rate_milli, estimate_remaining, percent_milli,
    };

    fn stats(bytes: u64, frames: u64) -> EncoderStats {
        EncoderStats {
            video_encoder: "x264enc".to_owned(),
            audio_encoder: None,
            muxer: "matroskamux".to_owned(),
            video_frames: frames,
            audio_frames: 0,
            bytes_written: bytes,
            encoded: RationalTime::from_frames(
                i64::try_from(frames).expect("a small frame count"),
                Rational::FPS_24,
            ),
        }
    }

    #[test]
    fn an_eta_needs_both_a_total_and_a_frame_to_extrapolate_from() {
        let elapsed = Duration::from_secs(1);
        assert_eq!(estimate_remaining(elapsed, 10, 0), None, "no total, no eta");
        assert_eq!(estimate_remaining(elapsed, 0, 100), None, "no rate yet");
        assert_eq!(
            estimate_remaining(elapsed, 100, 100),
            Some(Duration::ZERO),
            "a finished export has nothing left"
        );
        assert_eq!(
            estimate_remaining(elapsed, 200, 100),
            Some(Duration::ZERO),
            "an overrun never reports a negative wait"
        );
    }

    #[test]
    fn an_eta_scales_the_elapsed_time_by_the_work_left() {
        // A quarter done in one second means three more seconds.
        assert_eq!(
            estimate_remaining(Duration::from_secs(1), 25, 100),
            Some(Duration::from_secs(3))
        );
        // Half done in ten seconds means ten more.
        assert_eq!(
            estimate_remaining(Duration::from_secs(10), 50, 100),
            Some(Duration::from_secs(10))
        );
        // The arithmetic is exact in nanoseconds, not rounded through a float.
        assert_eq!(
            estimate_remaining(Duration::from_nanos(3), 1, 4),
            Some(Duration::from_nanos(9))
        );
    }

    #[test]
    fn the_encode_rate_is_frames_per_second_in_thousandths() {
        assert_eq!(encode_rate_milli(Duration::from_secs(1), 24), 24_000);
        assert_eq!(encode_rate_milli(Duration::from_secs(2), 24), 12_000);
        assert_eq!(encode_rate_milli(Duration::from_millis(500), 12), 24_000);
        assert_eq!(encode_rate_milli(Duration::ZERO, 12), 0, "no time, no rate");
        assert_eq!(encode_rate_milli(Duration::from_secs(1), 0), 0);
    }

    #[test]
    fn percentages_are_thousandths_and_never_pass_a_hundred() {
        assert_eq!(percent_milli(0, 0), None);
        assert_eq!(percent_milli(0, 100), Some(0));
        assert_eq!(percent_milli(1, 3), Some(33_333));
        assert_eq!(percent_milli(50, 100), Some(50_000));
        assert_eq!(percent_milli(100, 100), Some(100_000));
        assert_eq!(percent_milli(150, 100), Some(100_000));
    }

    #[test]
    fn a_progress_snapshot_reports_whole_percent_and_frames_left() {
        let progress = ExportProgress {
            frames_done: 30,
            frames_total: 120,
            elapsed: Duration::from_secs(2),
            eta: estimate_remaining(Duration::from_secs(2), 30, 120),
            frames_per_second_milli: encode_rate_milli(Duration::from_secs(2), 30),
            percent_milli: percent_milli(30, 120),
            stats: stats(4096, 30),
        };
        assert_eq!(progress.percent(), Some(25));
        assert_eq!(progress.frames_remaining(), Some(90));
        assert_eq!(progress.eta, Some(Duration::from_secs(6)));
        assert_eq!(progress.frames_per_second_milli, 15_000);

        let unknown = ExportProgress {
            frames_total: 0,
            percent_milli: None,
            eta: None,
            ..progress
        };
        assert_eq!(unknown.percent(), None);
        assert_eq!(unknown.frames_remaining(), None);
    }

    #[test]
    fn the_bitrate_comes_from_the_exact_media_duration() {
        // 24 frames at 24 fps is one second; 12_000 bytes is 96 kbit/s.
        assert_eq!(stats(12_000, 24).bits_per_second(), Some(96_000));
        assert_eq!(stats(0, 24).bits_per_second(), Some(0));
        assert_eq!(
            stats(12_000, 0).bits_per_second(),
            None,
            "no media time, no bitrate"
        );
    }
}
