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

use serde_json::{Value, json};
use sub_command::host::{ExportParams, ExportStatus, FrameImage, FrameRequest, Services};
use sub_core::{SubError, SubResult, codes};
use sub_export::{ExportEvent, PresetLibrary};
use sub_media::probe::{MediaInfo, probe};
use sub_media::proxy::{Proxy, ProxyCodec, ProxyOptions, proxy_size};
use sub_model::{MediaId, Project, json as project_json};

/// The folder proxies are written into, relative to the project.
pub const PROXY_DIR: &str = "proxies";

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
        let source = project
            .absolute_path(&self.project_dir, media)
            .ok_or_else(|| {
                SubError::new(codes::NOT_FOUND, "no such media item in the project")
                    .with_detail("media", media.to_string())
            })?;
        let info = probe(&source)?;
        let video = info.video.first().ok_or_else(|| {
            SubError::new(
                codes::INVALID_ARGUMENT,
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
        let cache = self.project_dir.join(PROXY_DIR);
        let proxy = Proxy::generate(&source, &cache, options)?;
        let file = proxy.path();
        let relative = file.strip_prefix(&self.project_dir).unwrap_or(&file);
        Ok(relative.to_string_lossy().replace('\\', "/"))
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
        Ok(FrameImage {
            data: base64(&frame.png),
            mime_type: "image/png".to_owned(),
            width: frame.width,
            height: frame.height,
            time: request.time,
        })
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

/// One preset, as `export.list_presets` reports it.
fn preset_json(preset: &sub_export::Preset) -> Value {
    json!({
        "id": preset.id,
        "name": preset.name,
        "container": preset.container,
        "video": preset.video.as_ref().map(|video| json!({
            "codec": video.codec.as_str(),
            "width": video.width,
            "height": video.height,
            "frame_rate": {
                "numerator": video.frame_rate.numerator(),
                "denominator": video.frame_rate.denominator(),
            },
            "quality": video.quality.to_string(),
        })),
        "audio": preset.audio.as_ref().map(|audio| json!({
            "codec": audio.codec.as_str(),
            "channels": audio.channels,
            "sample_rate": audio.sample_rate,
            "bitrate_kbps": audio.bitrate_kbps,
        })),
    })
}

/// A probed file, as `media.probe` reports it.
///
/// [`MediaInfo`] is not a serde type — it is what the prober returns, not
/// something the project file stores — so the wire shape is built here, and
/// every time in it is an exact rational.
fn media_info_json(info: &MediaInfo, path: &Path) -> Value {
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

/// Standard base64, as an MCP image content block carries a PNG.
///
/// Sixteen lines rather than a dependency: this is the only place in the build
/// that needs it, and the alphabet has not moved since RFC 4648.
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let triple = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                let shift = 18 - index * 6;
                out.push(char::from(ALPHABET[((triple >> shift) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{CliServices, base64};
    use std::path::Path;
    use sub_command::host::Services;

    #[test]
    fn base64_matches_the_rfc_test_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // The PNG signature, which is what a frame's data starts with.
        assert_eq!(base64(&[0x89, b'P', b'N', b'G']), "iVBORw==");
    }

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
