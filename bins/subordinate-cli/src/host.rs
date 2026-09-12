//! The headless implementation of [`sub_command::host::Services`].
//!
//! `sub-command` says what the host-backed tool families need
//! (docs/PLAN.md §7); this is what `subordinate-cli serve` supplies for them,
//! out of the same crates the `render` and `inspect` subcommands use:
//!
//! - `media.probe` is the GStreamer discoverer ([`sub_media::probe`]);
//! - `media.make_proxy` is the proxy transcoder ([`sub_media::Proxy`]), into a
//!   `proxies/` folder beside the project file, so the path recorded on the
//!   media item stays project-relative like every other path in the model;
//! - `playback.render_frame_png` is the compositor, stopped after one picture
//!   and encoded ([`crate::render::frame_png`]);
//! - `export.*` is the preset library and the export pipeline, run on a thread
//!   of its own so the call that starts a render returns at once and
//!   `export.progress` follows it.
//!
//! An export renders the project *as the engine holds it*, not as the file on
//! disk holds it: the snapshot is written to a scratch file beside the project
//! first, so an agent that has just made an edit exports that edit. The
//! scratch file sits in the project folder because media paths are stored
//! relative to it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::Value;
use sub_command::host::{ExportParams, ExportStatus, FrameImage, FrameRequest, Services};
use sub_core::{SubError, SubResult, codes};
use sub_export::{ExportEvent, PresetLibrary, preset_json};
use sub_media::probe::{media_info_json, probe};
use sub_media::proxy::proxy_for_media;
use sub_model::{MediaId, Project, json as project_json};

/// The host services of a headless server.
#[derive(Debug)]
pub struct CliServices {
    /// The folder the project file lives in, which every relative media path
    /// resolves against.
    project_dir: PathBuf,
    /// Every export this process has started, by job id.
    ///
    /// Shared rather than owned: each export runs on a thread of its own and
    /// writes its latest status here, where `export.progress` reads it.
    exports: Statuses,
    /// The source of job ids.
    next_job: AtomicU64,
}

/// The status table an export thread writes into.
type Statuses = Arc<Mutex<HashMap<String, ExportStatus>>>;

impl CliServices {
    /// Services for a server whose project file is `project`, or whose project
    /// is unsaved and so resolves media against the working directory.
    #[must_use]
    pub fn new(project: Option<&Path>) -> Self {
        let project_dir = project
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            project_dir,
            exports: Arc::new(Mutex::new(HashMap::new())),
            next_job: AtomicU64::new(1),
        }
    }

    /// Records `status` against its job id.
    fn record(&self, status: ExportStatus) {
        self.exports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(status.job.clone(), status);
    }
}

impl Services for CliServices {
    fn project_dir(&self) -> PathBuf {
        self.project_dir.clone()
    }

    fn probe(&self, path: &Path) -> SubResult<Value> {
        Ok(media_info_json(&probe(path)?, path))
    }

    fn make_proxy(&self, project: &Project, media: MediaId) -> SubResult<String> {
        proxy_for_media(project, &self.project_dir, media)
    }

    fn render_frame_png(&self, request: &FrameRequest<'_>) -> SubResult<FrameImage> {
        let sequence = request.project.sequence(request.sequence).ok_or_else(|| {
            SubError::new(codes::NOT_FOUND, "no such sequence in the project")
                .with_detail("sequence", request.sequence.to_string())
        })?;
        let frame = crate::render::frame_png(
            request.project,
            &self.project_dir,
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
        let library = PresetLibrary::load()?;
        Ok(library.iter().map(preset_json).collect())
    }

    fn start_export(&self, project: &Project, request: &ExportParams) -> SubResult<ExportStatus> {
        let sequence = sub_command::agent::pick_sequence(project, request.sequence)?;
        let job = format!("export-{}", self.next_job.fetch_add(1, Ordering::Relaxed));
        let scratch = self.project_dir.join(format!(".{job}.sub"));
        let text = project_json::to_json(project)?;
        std::fs::write(&scratch, text).map_err(|error| {
            SubError::new(codes::IO, "the export scratch project could not be written")
                .with_detail("path", scratch.display().to_string())
                .with_cause(&error)
        })?;

        let options = crate::render::Options {
            project: scratch.clone(),
            sequence: Some(sequence.name.clone()),
            preset: request.preset.clone(),
            output: request.output.clone(),
            encoder: None,
            range: request.range.map(|range| crate::render::FrameRange {
                start: range.start_frame,
                end: range.end_frame,
            }),
            verify: false,
        };
        let output = request.output.display().to_string();
        let started = ExportStatus::running(job.clone(), output.clone(), 0, 0);
        self.record(started.clone());

        let exports = Arc::new(Exports {
            job: job.clone(),
            output,
        });
        let statuses = Arc::clone(&self.exports);
        std::thread::Builder::new()
            .name(job.clone())
            .spawn(move || {
                let mut on_event = |event: &ExportEvent| {
                    let status = exports.status_of(event);
                    statuses
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(status.job.clone(), status);
                };
                let outcome = crate::render::run_with(&options, &mut on_event);
                if let Err(error) = outcome {
                    let status = exports.failed(&error);
                    statuses
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(status.job.clone(), status);
                }
                std::fs::remove_file(&scratch).ok();
            })
            .map_err(|error| {
                SubError::new(codes::INTERNAL, "the export thread could not be started")
                    .with_cause(&error)
            })?;
        Ok(started)
    }

    fn export_progress(&self, job: &str) -> SubResult<ExportStatus> {
        self.exports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(job)
            .cloned()
            .ok_or_else(|| {
                SubError::new(codes::NOT_FOUND, "no such export job")
                    .with_detail("job", job.to_owned())
            })
    }
}

/// One export's identity, as its thread reports on it.
#[derive(Debug)]
struct Exports {
    job: String,
    output: String,
}

impl Exports {
    /// The status an export event leaves the job in.
    fn status_of(&self, event: &ExportEvent) -> ExportStatus {
        match event {
            ExportEvent::Started { frames_total, .. } => {
                ExportStatus::running(&self.job, &self.output, 0, *frames_total)
            }
            ExportEvent::Progress(progress) => ExportStatus::running(
                &self.job,
                &self.output,
                progress.frames_done,
                progress.frames_total,
            ),
            ExportEvent::Finished(report) => ExportStatus::running(
                &self.job,
                &self.output,
                report.video_frames,
                report.video_frames,
            )
            .finished("completed"),
            ExportEvent::Cancelled { frames_done, .. } => {
                ExportStatus::running(&self.job, &self.output, *frames_done, 0)
                    .finished("cancelled")
            }
            ExportEvent::Failed {
                error, frames_done, ..
            } => ExportStatus::running(&self.job, &self.output, *frames_done, 0).failed(error),
        }
    }

    /// The status a render that never started leaves the job in.
    fn failed(&self, error: &SubError) -> ExportStatus {
        ExportStatus::running(&self.job, &self.output, 0, 0).failed(error)
    }
}

#[cfg(test)]
mod tests {
    use super::CliServices;
    use std::path::Path;
    use sub_command::host::Services;

    #[test]
    fn the_project_folder_is_where_media_paths_resolve() {
        let services = CliServices::new(Some(Path::new("/projects/doc/doc.sub")));
        assert_eq!(services.project_dir(), Path::new("/projects/doc"));
    }

    #[test]
    fn an_unknown_export_job_is_a_stable_not_found() {
        let services = CliServices::new(None);
        let error = services
            .export_progress("export-9")
            .expect_err("no such job");
        assert_eq!(error.code.as_str(), "core.not_found");
    }
}
