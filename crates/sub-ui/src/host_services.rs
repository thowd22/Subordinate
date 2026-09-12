//! The window's own [`sub_command::host::Services`]: export, probe and frame
//! methods answered by the editor the user is looking at (TASK-155).
//!
//! [`crate::session`] puts the engine families — `project.*`, `timeline.*`,
//! `playback.*`, `media.list`, `history.*` — on the dispatcher the socket
//! serves. The other six methods need more than the engine (docs/PLAN.md §7),
//! and until this module existed the running editor answered them with
//! `command.unknown_method`: an agent could rename a sequence in the window it
//! could see and then had to start a second headless engine to export it.
//!
//! What makes the editor's implementation different from the CLI's
//! ([`CliServices`](../../subordinate_cli/host/struct.CliServices.html)) is
//! *whose* GPU and *whose* job pool the work runs on:
//!
//! - `playback.render_frame_png` composites on the window's own
//!   [`RenderContext`] — the very `wgpu::Device` egui paints with — so an
//!   agent's picture comes off the compositor the viewer draws with rather
//!   than off a second headless context;
//! - `export.render` is run by the window's [`ExportRunner`](crate::ExportRunner)
//!   on the window's [`JobService`](sub_core::JobService), through the export
//!   panel's own request, so the export an agent starts shows up in the panel
//!   with its progress, its ETA and its Cancel button;
//! - `export.list_presets` reads the library the panel offers, so the ids an
//!   agent is told about are the ids the window can resolve.
//!
//! **Nothing here ever blocks on the UI thread.** A socket thread that waited
//! for a frame would deadlock any caller that is itself driving the window (an
//! `egui_kittest` test is exactly that), so `export.render` validates what it
//! can on the calling thread — the preset, the sequence, whether a project
//! file exists, whether an export is already running — allocates a job id,
//! queues the request and returns at once. The window drains that queue once a
//! frame in [`crate::app`], starts the export the same way a click on Export
//! starts one, and publishes what the panel is showing back into the table
//! `export.progress` reads. `media.probe`, `media.make_proxy` and
//! `playback.render_frame_png` run on the calling socket thread, against
//! handles (`wgpu::Device`, `wgpu::Queue`) that are shared rather than owned by
//! the UI thread — so the window keeps painting while any of them runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use eframe::egui;
use serde_json::Value;
use sub_command::agent::pick_sequence;
use sub_command::host::{ExportParams, ExportStatus, FrameImage, FrameRequest, Services};
use sub_core::{SubError, SubResult, codes};
use sub_export::{PresetLibrary, preset_json};
use sub_media::{media_info_json, probe, proxy_for_media};
use sub_model::{MediaId, Project, ProjectId};
use sub_render::RenderContext;

use crate::export_panel::ExportStatus as PanelStatus;

/// One export an agent asked for, waiting for the window to start it.
#[derive(Debug, Clone)]
pub struct QueuedExport {
    /// The job id `export.render` already answered with.
    pub job: String,
    /// What was asked for.
    pub params: ExportParams,
    /// The immutable project accepted by the socket call.
    pub project: Arc<Project>,
    /// Where that project resolves its media, even if another project is opened.
    pub project_dir: PathBuf,
}

/// The state the window and the socket threads share.
///
/// Every field is behind its own lock and none of them is held across a call
/// into the engine, the encoder or egui: a socket thread reads and writes them
/// while the UI thread is painting.
pub struct GuiHost {
    /// The folder the open project file lives in, as the window last knew it.
    project_dir: Mutex<Option<PathBuf>>,
    project_dirs: Mutex<HashMap<ProjectId, Option<PathBuf>>>,
    /// Exports asked for over the socket and not yet started by the window.
    queue: Mutex<Vec<QueuedExport>>,
    /// Every export this window has started over the socket, by job id.
    exports: Mutex<HashMap<String, ExportStatus>>,
    /// The job the window is running for the socket right now.
    running: Mutex<Option<String>>,
    /// The source of job ids.
    next_job: AtomicU64,
    /// The egui context, so a queued export wakes the window instead of
    /// waiting for the user to move the pointer.
    repaint: Mutex<Option<egui::Context>>,
}

impl std::fmt::Debug for GuiHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuiHost")
            .field("project_dir", &self.project_dir())
            .field("queued", &self.queued_len())
            .field("running", &self.running_job())
            .finish_non_exhaustive()
    }
}

impl Default for GuiHost {
    fn default() -> Self {
        Self::new()
    }
}

impl GuiHost {
    /// A bridge with nothing queued and nothing running.
    #[must_use]
    pub fn new() -> Self {
        Self {
            project_dir: Mutex::new(None),
            project_dirs: Mutex::new(HashMap::new()),
            queue: Mutex::new(Vec::new()),
            exports: Mutex::new(HashMap::new()),
            running: Mutex::new(None),
            next_job: AtomicU64::new(1),
            repaint: Mutex::new(None),
        }
    }

    /// Locks `slot`, taking a poisoned lock's value rather than panicking: a
    /// worker that died holding one must not stop the window answering.
    fn lock<T>(slot: &Mutex<T>) -> MutexGuard<'_, T> {
        slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Tells the bridge where the open project file lives, or that there is no
    /// file yet. Called by the window once a frame.
    pub fn set_project_dir(&self, project: ProjectId, dir: Option<PathBuf>) {
        Self::lock(&self.project_dirs).insert(project, dir.clone());
        *Self::lock(&self.project_dir) = dir;
    }

    /// The folder media paths resolve against, when the project has a file.
    #[must_use]
    pub fn project_dir(&self) -> Option<PathBuf> {
        Self::lock(&self.project_dir).clone()
    }

    /// Remembers the context to wake when something is queued.
    pub fn set_repaint(&self, ctx: &egui::Context) {
        let mut slot = Self::lock(&self.repaint);
        if slot.is_none() {
            *slot = Some(ctx.clone());
        }
    }

    /// How many exports are waiting for the window to start them.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        Self::lock(&self.queue).len()
    }

    /// The job the window is running for the socket, if it is running one.
    #[must_use]
    pub fn running_job(&self) -> Option<String> {
        Self::lock(&self.running).clone()
    }

    /// Whether an export asked for over the socket is queued or running.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.running_job().is_some() || self.queued_len() > 0
    }

    /// Queues `params` as a new job and answers with its first status.
    ///
    /// The status is `running` with no frames yet: the window has not started
    /// the export, and `export.progress` is what follows it from here.
    ///
    /// # Errors
    ///
    /// [`crate::codes::EXPORT_BUSY`] if another client already reserved an export.
    pub fn queue_export(
        &self,
        params: ExportParams,
        project: Arc<Project>,
        project_dir: PathBuf,
    ) -> SubResult<ExportStatus> {
        let mut running = Self::lock(&self.running);
        if running.is_some() {
            return Err(SubError::new(
                crate::codes::EXPORT_BUSY,
                "this editor is already exporting",
            ));
        }
        let job = format!("export-{}", self.next_job.fetch_add(1, Ordering::Relaxed));
        let status = ExportStatus::running(job.clone(), params.output.display().to_string(), 0, 0);
        self.record(status.clone());
        *running = Some(job.clone());
        Self::lock(&self.queue).push(QueuedExport {
            job,
            params,
            project,
            project_dir,
        });
        if let Some(ctx) = Self::lock(&self.repaint).as_ref() {
            ctx.request_repaint();
        }
        Ok(status)
    }

    /// Takes everything queued, for the window to start. Once a frame.
    pub fn take_queued(&self) -> Vec<QueuedExport> {
        std::mem::take(&mut *Self::lock(&self.queue))
    }

    /// Records `status` against its job id.
    pub fn record(&self, status: ExportStatus) {
        Self::lock(&self.exports).insert(status.job.clone(), status);
    }

    /// What `job` is doing, if this window has ever started it.
    #[must_use]
    pub fn status(&self, job: &str) -> Option<ExportStatus> {
        Self::lock(&self.exports).get(job).cloned()
    }

    /// Records that the export the window was running for the socket is over.
    pub fn finish_running(&self) {
        *Self::lock(&self.running) = None;
    }

    /// Folds what the export panel is showing into the table
    /// `export.progress` reads.
    ///
    /// Called by the window once a frame while an export asked for over the
    /// socket is running. A terminal state is recorded once and then the job
    /// is let go, so the next export over the socket starts clean.
    pub fn publish(&self, panel: &PanelStatus) {
        let Some(job) = self.running_job() else {
            return;
        };
        let previous = self.status(&job);
        let Some(status) = wire_status(&job, panel, previous.as_ref()) else {
            return;
        };
        let running = status.state == "running";
        self.record(status);
        if !running {
            self.finish_running();
        }
    }

    /// Records that starting `job` was refused, with why.
    pub fn refuse(&self, job: &str, error: &SubError) {
        let previous = self.status(job);
        let output = previous.map(|status| status.output).unwrap_or_default();
        self.record(ExportStatus::running(job, output, 0, 0).failed(error));
        self.finish_running();
    }
}

/// The wire status a panel status describes, or `None` when the panel has
/// nothing new to say about the job.
///
/// The panel is the window's own view of the running export, so this is a
/// translation rather than a second count: the frames, the total and the
/// failure all come from the job's own events. `previous` supplies the output
/// path for the states that do not carry one.
fn wire_status(
    job: &str,
    panel: &PanelStatus,
    previous: Option<&ExportStatus>,
) -> Option<ExportStatus> {
    let output = || {
        previous
            .map(|status| status.output.clone())
            .unwrap_or_default()
    };
    let total = previous.map_or(0, |status| status.frames_total);
    match panel {
        // The window has not started it yet, or the job has not posted its
        // first event: the status queued with the job still stands.
        PanelStatus::Idle => None,
        PanelStatus::Running {
            path,
            frames_total,
            progress,
            ..
        } => Some(ExportStatus::running(
            job,
            path.display().to_string(),
            progress.as_ref().map_or(0, |progress| progress.frames_done),
            *frames_total,
        )),
        PanelStatus::Finished(report) => Some(
            ExportStatus::running(
                job,
                report.path.display().to_string(),
                report.video_frames,
                report.video_frames,
            )
            .finished("completed"),
        ),
        PanelStatus::Cancelled { frames_done } => {
            Some(ExportStatus::running(job, output(), *frames_done, total).finished("cancelled"))
        }
        PanelStatus::Failed { error, frames_done } => {
            Some(ExportStatus::running(job, output(), *frames_done, total).failed(error))
        }
    }
}

/// The editor's host services: the six methods an engine cannot answer.
#[derive(Debug)]
pub struct GuiServices {
    /// What the window and these methods share.
    host: Arc<GuiHost>,
    /// The window's render context — egui's own device and queue.
    render: RenderContext,
    /// The presets the export panel offers.
    presets: Arc<PresetLibrary>,
}

impl GuiServices {
    /// Services over `host`, drawing on `render` and resolving `presets`.
    #[must_use]
    pub fn new(host: Arc<GuiHost>, render: RenderContext, presets: Arc<PresetLibrary>) -> Self {
        Self {
            host,
            render,
            presets,
        }
    }

    /// The bridge the window pumps.
    #[must_use]
    pub fn host(&self) -> &Arc<GuiHost> {
        &self.host
    }

    /// The project folder, or the "this project has no file yet" refusal.
    ///
    /// Media paths are project-relative (docs/PLAN.md §5.6), so a project that
    /// has never been saved can neither be exported nor have a proxy made for
    /// it — the same refusal the panel shows.
    fn require_project_dir(&self, project: &Project) -> SubResult<PathBuf> {
        GuiHost::lock(&self.host.project_dirs)
            .get(&project.id)
            .cloned()
            .flatten()
            .ok_or_else(|| {
                SubError::new(
                    crate::codes::EXPORT_NOT_READY,
                    "this project has no file yet; save it so its media paths resolve",
                )
                .with_detail("field", "project")
            })
    }
}

impl Services for GuiServices {
    fn project_dir(&self) -> PathBuf {
        self.host
            .project_dir()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn project_dir_for(&self, project: &Project) -> SubResult<PathBuf> {
        self.require_project_dir(project)
    }

    fn project_file_changed(&self, project: &Project, path: Option<&Path>) {
        let dir = path
            .and_then(|path| path.parent())
            .and_then(|dir| {
                std::fs::canonicalize(if dir.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    dir
                })
                .ok()
            })
            .map(sub_model::plain_path);
        self.host.set_project_dir(project.id, dir);
    }

    fn probe(&self, path: &Path) -> SubResult<Value> {
        Ok(media_info_json(&probe(path)?, path))
    }

    fn make_proxy(&self, project: &Project, media: MediaId) -> SubResult<String> {
        proxy_for_media(project, &self.require_project_dir(project)?, media)
    }

    fn render_frame_png(&self, request: &FrameRequest<'_>) -> SubResult<FrameImage> {
        let sequence = request.project.sequence(request.sequence).ok_or_else(|| {
            SubError::new(codes::NOT_FOUND, "no such sequence in the project")
                .with_detail("sequence", request.sequence.to_string())
        })?;
        let frame = sub_export::sequence::frame_png(
            &self.render,
            request.project,
            &self.require_project_dir(request.project)?,
            sequence,
            request.time,
            request.width,
        )?;
        Ok(FrameImage::png(
            &frame.png,
            frame.width,
            frame.height,
            request.time,
        ))
    }

    fn presets(&self) -> SubResult<Vec<Value>> {
        Ok(self.presets.iter().map(preset_json).collect())
    }

    fn start_export(&self, project: &Project, request: &ExportParams) -> SubResult<ExportStatus> {
        // Everything that can be refused without the UI thread is refused
        // here, where the caller sees it as the error of its own call rather
        // than as a failed job it has to poll for.
        self.presets.require(&request.preset)?;
        pick_sequence(project, request.sequence)?;
        let project_dir = self.require_project_dir(project)?;
        self.host
            .queue_export(request.clone(), Arc::new(project.clone()), project_dir)
    }

    fn export_progress(&self, job: &str) -> SubResult<ExportStatus> {
        self.host.status(job).ok_or_else(|| {
            SubError::new(codes::NOT_FOUND, "no such export job").with_detail("job", job.to_owned())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{GuiHost, PanelStatus, wire_status};
    use std::path::PathBuf;
    use sub_command::host::ExportParams;
    use sub_core::SubError;
    use sub_export::{EncoderStats, ExportReport};
    use sub_time::{Rational, RationalTime};

    /// An export of `output`, with no sequence and no range: the shape of what
    /// arrives over the socket.
    fn params(output: &str) -> ExportParams {
        ExportParams {
            preset: "mezzanine".to_owned(),
            output: PathBuf::from(output),
            sequence: None,
            range: None,
        }
    }

    #[test]
    fn a_queued_export_answers_at_once_and_makes_the_window_busy() {
        let host = GuiHost::new();
        assert!(!host.is_busy(), "nothing is running to begin with");

        let status = host
            .queue_export(
                params("/tmp/out.mkv"),
                std::sync::Arc::new(sub_model::Project::new("test")),
                PathBuf::from("/tmp"),
            )
            .unwrap();
        assert_eq!(status.state, "running");
        assert_eq!(status.job, "export-1");
        assert_eq!(status.output, "/tmp/out.mkv");
        assert_eq!(status.frames_done, 0);
        assert!(host.is_busy(), "the window has an export to start");
        assert_eq!(
            host.status("export-1").expect("the job is known").state,
            "running",
            "export.progress answers before the window has painted a frame",
        );

        let queued = host.take_queued();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].job, "export-1");
        assert_eq!(queued[0].params.output, PathBuf::from("/tmp/out.mkv"));
        assert!(host.take_queued().is_empty(), "taken once, not twice");
    }

    #[test]
    fn concurrent_clients_reserve_only_one_export() {
        let host = std::sync::Arc::new(GuiHost::new());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let host = host.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    host.queue_export(
                        params("/tmp/out.mkv"),
                        std::sync::Arc::new(sub_model::Project::new("test")),
                        PathBuf::from("/tmp"),
                    )
                })
            })
            .collect();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        for error in results.into_iter().filter_map(Result::err) {
            assert_eq!(error.code, crate::codes::EXPORT_BUSY);
        }
        assert_eq!(host.queued_len(), 1);
    }

    #[test]
    fn an_unknown_job_is_a_stable_not_found() {
        let host = GuiHost::new();
        host.queue_export(
            params("/tmp/out.mkv"),
            std::sync::Arc::new(sub_model::Project::new("test")),
            PathBuf::from("/tmp"),
        )
        .unwrap();
        assert!(host.status("export-9").is_none());
    }

    #[test]
    fn the_panels_progress_is_what_export_progress_reports() {
        let host = GuiHost::new();
        host.queue_export(
            params("/tmp/out.mkv"),
            std::sync::Arc::new(sub_model::Project::new("test")),
            PathBuf::from("/tmp"),
        )
        .unwrap();

        host.take_queued();
        host.publish(&PanelStatus::Idle);
        let waiting = host.status("export-1").expect("the job");
        assert_eq!(waiting.frames_total, 0, "the window has not started it yet");

        host.publish(&PanelStatus::Running {
            path: PathBuf::from("/tmp/out.mkv"),
            frames_total: 12,
            progress: None,
            stats: Box::new(EncoderStats {
                video_encoder: String::new(),
                audio_encoder: None,
                muxer: String::new(),
                video_frames: 0,
                audio_frames: 0,
                bytes_written: 0,
                encoded: RationalTime::from_frames(0, Rational::FPS_30),
            }),
        });
        let started = host.status("export-1").expect("the job");
        assert_eq!(started.state, "running");
        assert_eq!(started.frames_total, 12);
        assert_eq!(started.frames_done, 0);
        assert!(host.is_busy(), "it is still running");

        host.publish(&PanelStatus::Finished(Box::new(ExportReport {
            path: PathBuf::from("/tmp/out.mkv"),
            video_frames: 12,
            audio_frames: 0,
            duration: RationalTime::new(12, Rational::FPS_25),
            video_encoder: "x264enc".to_owned(),
            audio_encoder: None,
            muxer: "matroskamux".to_owned(),
        })));
        let done = host.status("export-1").expect("the job");
        assert_eq!(done.state, "completed");
        assert_eq!(done.frames_done, 12);
        assert_eq!(done.permille, 1000);
        assert!(!host.is_busy(), "the window is free for the next one");
    }

    #[test]
    fn a_failed_export_carries_the_error_the_panel_shows() {
        let host = GuiHost::new();
        host.queue_export(
            params("/tmp/out.mkv"),
            std::sync::Arc::new(sub_model::Project::new("test")),
            PathBuf::from("/tmp"),
        )
        .unwrap();
        host.take_queued();
        let error = SubError::new(sub_core::codes::INTERNAL, "the encoder stopped");
        host.publish(&PanelStatus::Failed {
            error: Box::new(error.clone()),
            frames_done: 3,
        });
        let status = host.status("export-1").expect("the job");
        assert_eq!(status.state, "failed");
        assert_eq!(status.frames_done, 3);
        assert_eq!(status.error, Some(error.to_json()));
        assert!(!host.is_busy());
    }

    #[test]
    fn a_refused_start_is_reported_on_the_job_it_was_given() {
        let host = GuiHost::new();
        host.queue_export(
            params("/tmp/out.mkv"),
            std::sync::Arc::new(sub_model::Project::new("test")),
            PathBuf::from("/tmp"),
        )
        .unwrap();
        host.take_queued();
        let error = SubError::new(crate::codes::EXPORT_NOT_READY, "choose an export preset");
        host.refuse("export-1", &error);
        let status = host.status("export-1").expect("the job");
        assert_eq!(status.state, "failed");
        assert_eq!(status.error, Some(error.to_json()));
        assert!(!host.is_busy(), "a refusal frees the window too");
    }

    #[test]
    fn a_cancelled_export_keeps_the_output_it_was_asked_for() {
        let status = wire_status(
            "export-1",
            &PanelStatus::Cancelled { frames_done: 4 },
            Some(&sub_command::host::ExportStatus::running(
                "export-1",
                "/tmp/out.mkv",
                0,
                12,
            )),
        )
        .expect("a cancel is news");
        assert_eq!(status.state, "cancelled");
        assert_eq!(status.frames_done, 4);
        assert_eq!(status.frames_total, 12);
        assert_eq!(status.output, "/tmp/out.mkv");
    }
}
