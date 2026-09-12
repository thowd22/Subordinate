//! The piece between the export panel and the export job (TASK-62,
//! docs/PLAN.md §5.5).
//!
//! [`ExportPanel`](crate::export_panel::ExportPanel) knows what the user asked
//! for and how to show what an export is doing; [`sub_export`] knows how to
//! render one. This module is the wire between them: it turns an
//! [`ExportRequest`] into an [`ExportJob`], spawns it on the shared
//! [`JobService`], drains the job's [`ExportEvent`] stream into the panel once
//! a frame, and hands the panel's Cancel button the job's cancel token.
//!
//! Nothing here decides where the pixels come from. The host supplies them
//! through [`ExportSources`], because the pixels of a real export are the
//! compositor's full-resolution readback and the offline audio mix, which live
//! on the other side of the render context; a test supplies synthetic frames
//! through the same trait.
//!
//! No mutation of the project happens here, so nothing in this module is a
//! Command: an export reads the project and writes a file.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use sub_core::JobService;
//! use sub_export::{PresetLibrary, SolidFrames};
//! use sub_ui::export_panel::ExportPanel;
//! use sub_ui::export_runner::{ExportRunner, ExportStreams};
//!
//! # let request = unimplemented!();
//! # let project = sub_model::Project::new("demo");
//! let jobs = JobService::new(1);
//! let library = PresetLibrary::builtin();
//! let mut panel = ExportPanel::new();
//! let mut runner = ExportRunner::new();
//!
//! runner.start(&jobs, &library, &project, &request, &mut |_: &_, settings: &sub_export::ExportSettings| {
//!     Ok(ExportStreams::video(Box::new(SolidFrames::new(
//!         settings.frame_bytes(),
//!         240,
//!     ))))
//! })?;
//! // Once a frame, for as long as the export runs:
//! runner.poll(&mut panel);
//! # Ok(())
//! # }
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use sub_core::{JobService, Priority, SubError, SubResult};
use sub_export::{
    AudioFrameSource, EncoderPreferences, ExportElements, ExportEvent, ExportJob, ExportJobHandle,
    ExportSettings, PresetLibrary, VideoFrameSource, spawn_export_job,
};
use sub_model::Project;
use sub_render::RenderContext;
use sub_time::Rounding;

use crate::codes;
use crate::export_panel::{ExportPanel, ExportRequest, PresetSource};

/// The priority an export runs at.
///
/// An export is what the person is standing there waiting for, but it is a
/// long grind rather than a gesture: [`Priority::Normal`] keeps the
/// interactive work — the strip under the pointer, the waveform being
/// scrubbed — ahead of it.
pub const EXPORT_PRIORITY: Priority = Priority::Normal;

/// The streams one export reads.
///
/// The video source is required: the export pipeline is driven by composited
/// frames, and it is the frame source running dry that ends the export. The
/// audio source is `None` for a silent export, and short audio is padded with
/// silence by the job rather than ending the file early.
pub struct ExportStreams {
    /// Where the composited frames come from.
    pub video: Box<dyn VideoFrameSource + Send>,
    /// Where the mixed samples come from, when the export has audio.
    pub audio: Option<Box<dyn AudioFrameSource + Send>>,
}

impl std::fmt::Debug for ExportStreams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportStreams")
            .field("audio", &self.audio.is_some())
            .finish_non_exhaustive()
    }
}

impl ExportStreams {
    /// Streams with video only: a silent export.
    #[must_use]
    pub fn video(video: Box<dyn VideoFrameSource + Send>) -> Self {
        Self { video, audio: None }
    }

    /// The same streams with `audio` alongside the picture.
    #[must_use]
    pub fn with_audio(mut self, audio: Box<dyn AudioFrameSource + Send>) -> Self {
        self.audio = Some(audio);
        self
    }
}

/// Where an export's pixels and samples come from.
///
/// The host implements this over its render context; any
/// `FnMut(&ExportRequest, &ExportSettings) -> SubResult<ExportStreams>` is one
/// too, which is what tests and a headless caller use. It is called on the UI
/// thread, before the job is queued, and the streams it returns are moved to
/// the worker: opening the sources is the host's last chance to refuse an
/// export it cannot serve.
pub trait ExportSources {
    /// Opens the streams for `request`, which will be encoded as `settings`.
    ///
    /// # Errors
    ///
    /// Whatever the host raises when it cannot read the sequence: a missing
    /// media file, a render context that has gone away, a decoder that will
    /// not start.
    fn open(
        &mut self,
        request: &ExportRequest,
        settings: &ExportSettings,
    ) -> SubResult<ExportStreams>;
}

impl<F> ExportSources for F
where
    F: FnMut(&ExportRequest, &ExportSettings) -> SubResult<ExportStreams>,
{
    fn open(
        &mut self,
        request: &ExportRequest,
        settings: &ExportSettings,
    ) -> SubResult<ExportStreams> {
        self(request, settings)
    }
}

/// Where an export reads its pixels and samples from (TASK-135).
///
/// The picture is the compositor's full-resolution readback (TASK-58) and the
/// sound is the mixer's offline render (TASK-55), built by
/// [`sub_export::sequence`] — the same adapters `subordinate-cli render`
/// writes its file through, so a GUI export and a CLI render of the same
/// project and preset are the same render.
///
/// Both halves are lazy, so this call opens no decoder, composites no frame
/// and mixes no sample: it hands the worker something that will, and the UI
/// thread goes straight back to painting. The wgpu device is shared with egui
/// — `Device` and `Queue` are `Send + Sync` handles to the one device — so the
/// export's readbacks queue alongside the window's own work rather than
/// stopping it.
///
/// `project_dir` is where a clip's relative media path resolves, which is the
/// folder the project file lives in; a project that has never been saved has
/// no such folder and cannot be exported.
pub fn sequence_sources(
    render: &RenderContext,
    project: Arc<Project>,
    project_dir: PathBuf,
) -> impl ExportSources {
    let render = render.clone();
    move |request: &ExportRequest, settings: &ExportSettings| -> SubResult<ExportStreams> {
        let sequence = project
            .sequences
            .iter()
            .find(|sequence| sequence.id == request.sequence)
            .ok_or_else(|| {
                SubError::new(
                    sub_core::codes::NOT_FOUND,
                    "the sequence this export was asked for is no longer open",
                )
            })?
            .clone();
        for warning in
            sub_export::unused_audio_streams_for_export(&project, &sequence, &project_dir)
        {
            log::warn!("export: {warning}");
        }
        let span = sub_export::FrameSpan::new(
            request.span.start().value(),
            u64::try_from(request.span.duration().value()).unwrap_or(0),
        );
        let (video, audio) = sub_export::open_streams(
            &render,
            Arc::clone(&project),
            Arc::new(sequence),
            &project_dir,
            settings,
            span,
        );
        let streams = ExportStreams::video(Box::new(video));
        Ok(match audio {
            Some(audio) => streams.with_audio(Box::new(audio)),
            None => streams,
        })
    }
}

/// The export the window is running, if it is running one.
///
/// At most one export runs at a time: they are long, they saturate an encoder,
/// and a second one asked for by accident would fight the first for it.
#[derive(Debug, Default)]
pub struct ExportRunner {
    /// The running job, or `None` when nothing is running.
    running: Option<ExportJobHandle>,
}

impl ExportRunner {
    /// A runner with nothing running.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether an export is running right now.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// The running job's handle, for a host that wants its state or priority.
    #[must_use]
    pub const fn handle(&self) -> Option<&ExportJobHandle> {
        self.running.as_ref()
    }

    /// Starts `request` on `jobs`, reading its streams from `sources`.
    ///
    /// Returns as soon as the job is queued. Everything the panel shows from
    /// then on — the progress bar, the percentage, the rate and the ETA —
    /// arrives through [`ExportRunner::poll`].
    ///
    /// # Errors
    ///
    /// [`codes::EXPORT_BUSY`] when an export is already running,
    /// [`codes::EXPORT_PRESET_UNSUPPORTED`] for a plugin preset, which no host
    /// can resolve to settings yet, the preset and sequence errors of
    /// [`settings_for`], the encoder errors of [`ExportElements::resolve`],
    /// and whatever `sources` raises.
    pub fn start(
        &mut self,
        jobs: &JobService,
        library: &PresetLibrary,
        project: &Project,
        request: &ExportRequest,
        sources: &mut dyn ExportSources,
    ) -> SubResult<()> {
        if self.is_running() {
            return Err(SubError::new(
                codes::EXPORT_BUSY,
                "an export is already running; cancel it before starting another",
            ));
        }
        let settings = settings_for(library, project, request)?;
        let elements = elements_for(&settings, request)?;
        let streams = sources.open(request, &settings)?;
        let job = ExportJob::new(&request.output, &settings, &elements)
            .with_total_frames(frames_total(request, &settings));
        self.running = Some(spawn_export_job(
            jobs,
            job,
            streams.video,
            streams.audio,
            EXPORT_PRIORITY,
        ));
        Ok(())
    }

    /// Hands every event the running export has posted to `panel`.
    ///
    /// Call once a frame. Returns how many events were applied, which is what
    /// tells a host whether the panel needs repainting. The handle is dropped
    /// on the job's terminal event, so a runner is ready for the next export
    /// the moment the panel has been told how the last one ended.
    pub fn poll(&mut self, panel: &mut ExportPanel) -> usize {
        let Some(handle) = &self.running else {
            return 0;
        };
        let mut applied = 0;
        let mut ended = false;
        while let Ok(event) = handle.events().try_recv() {
            ended |= is_terminal(&event);
            panel.apply_event(&event);
            applied += 1;
        }
        // A job that ended without a terminal event — a panicking worker is
        // the only way — must not leave the runner forever busy.
        if ended || handle.handle().is_finished() {
            self.running = None;
        }
        applied
    }

    /// Asks the running export to stop at the next frame boundary.
    ///
    /// This is what the panel's Cancel button reaches. The job deletes its
    /// part-written file and posts [`ExportEvent::Cancelled`], which the next
    /// [`ExportRunner::poll`] folds into the panel; the runner stays busy
    /// until then, because the export is still running until it says so.
    pub fn cancel(&self) {
        if let Some(handle) = &self.running {
            handle.cancel();
        }
    }
}

/// Whether `event` is the last one an export posts.
fn is_terminal(event: &ExportEvent) -> bool {
    matches!(
        event,
        ExportEvent::Finished(_) | ExportEvent::Cancelled { .. } | ExportEvent::Failed { .. }
    )
}

/// The settings `request` describes, resolved against `library` over the
/// sequence it names in `project`.
///
/// The preset chooses the container, the codecs and the audio format; the
/// sequence chooses the canvas and the timebase, because what is encoded is
/// the compositor's readback at the sequence's own resolution and nothing
/// rescales a finished frame. That is `sub_export::settings_for_sequence`,
/// which is also what `subordinate-cli render` resolves through, so a GUI
/// export and a CLI render of the same project write the same file.
///
/// # Errors
///
/// [`codes::EXPORT_PRESET_UNSUPPORTED`] for a preset an exporter plugin
/// contributed: the host has no way to turn one into settings yet, and an
/// export that silently used a different preset would be worse than a refusal.
/// [`sub_core::codes::NOT_FOUND`] when the project has lost the sequence the
/// request names. Otherwise the errors of [`PresetLibrary::require`] and
/// [`sub_export::settings_for_sequence`].
pub fn settings_for(
    library: &PresetLibrary,
    project: &Project,
    request: &ExportRequest,
) -> SubResult<ExportSettings> {
    if let PresetSource::Plugin { plugin } = &request.preset.source {
        return Err(SubError::new(
            codes::EXPORT_PRESET_UNSUPPORTED,
            format!(
                "preset '{}' comes from plugin {plugin}, which cannot render an export yet",
                request.preset.id
            ),
        )
        .with_detail("preset", request.preset.id.clone())
        .with_detail("plugin", plugin.to_string()));
    }
    let preset = library.require(&request.preset.id)?;
    let sequence = project
        .sequences
        .iter()
        .find(|sequence| sequence.id == request.sequence)
        .ok_or_else(|| {
            SubError::new(
                sub_core::codes::NOT_FOUND,
                "the sequence this export was asked for is no longer open",
            )
        })?;
    let (settings, mut warnings) = sub_export::settings_for_sequence(preset, sequence)?;
    // A source with several audio streams is cut on one of them; an export
    // that leaves the rest behind says so (TASK-153).
    warnings.extend(sub_export::unused_audio_streams(project, sequence));
    for warning in warnings {
        log::info!("export: {warning}");
    }
    Ok(settings)
}

/// The elements `request` will encode with: the plan's order, or the element
/// the user pinned in the panel.
///
/// # Errors
///
/// The errors of [`EncoderPreferences::set_override`] for an element that is
/// not catalogued for the preset's codec, and of [`ExportElements::resolve`]
/// when this machine has no usable encoder or muxer for the settings.
pub fn elements_for(
    settings: &ExportSettings,
    request: &ExportRequest,
) -> SubResult<ExportElements> {
    let mut preferences = EncoderPreferences::new();
    if let Some(element) = &request.encoder_override {
        preferences.set_override(settings.video_codec, element)?;
    }
    ExportElements::resolve(settings, &preferences)
}

/// How many frames `request` writes at the preset's frame rate.
///
/// The request's span is in the sequence's own frames; a preset may encode at
/// another rate, and then the count is the span rescaled to it. The rescale
/// rounds up so a span that does not land on a whole frame of the new rate is
/// covered rather than clipped, and it is exact rational arithmetic: no float
/// ever decides how long an export is.
#[must_use]
pub fn frames_total(request: &ExportRequest, settings: &ExportSettings) -> u64 {
    let duration = request.span.duration();
    let frames = if duration.rate() == settings.frame_rate {
        duration.value()
    } else {
        duration
            .rescaled_to_rounding(settings.frame_rate, Rounding::Ceil)
            .value()
    };
    u64::try_from(frames).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{ExportRunner, ExportStreams, elements_for, frames_total, settings_for};
    use crate::codes;
    use crate::export_panel::{ExportRange, ExportRequest, PresetEntry, PresetSource};
    use std::path::PathBuf;
    use sub_core::JobService;
    use sub_export::{ExportSettings, PresetLibrary, SolidFrames, VideoCodec};
    use sub_model::sequence::SequenceSettings;
    use sub_model::{ColorTags, Project, Resolution, Sequence};
    use sub_plugin::manifest::PluginId;
    use sub_time::{Rational, RationalTime, TimeRange};

    /// A project holding one sequence at `rate`, and a request for the first
    /// built-in preset over `frames` of it.
    ///
    /// The settings an export resolves come from the sequence's own canvas and
    /// timebase, so a request is only meaningful next to the project it names.
    fn project_at(rate: Rational, frames: i64) -> (Project, ExportRequest) {
        let library = PresetLibrary::builtin();
        let preset = library.iter().next().expect("a built-in preset");
        let settings = SequenceSettings::new(
            Resolution::new(640, 480).expect("a valid canvas"),
            rate,
            48_000,
            ColorTags::default(),
        )
        .expect("valid sequence settings");
        let sequence = Sequence::new("Main", settings);
        let id = sequence.id;
        let mut project = Project::new("demo");
        project.sequences.push(sequence);
        let request = ExportRequest {
            preset: PresetEntry::from_preset(preset),
            sequence: id,
            range: ExportRange::WholeSequence,
            span: TimeRange::new(
                RationalTime::from_frames(0, rate),
                RationalTime::from_frames(frames, rate),
            )
            .expect("a non-negative span"),
            output: PathBuf::from("out.mp4"),
            encoder_override: None,
        };
        (project, request)
    }

    fn project() -> (Project, ExportRequest) {
        project_at(Rational::FPS_24, 48)
    }

    /// Settings at `rate`, for the frame-count arithmetic on its own.
    fn settings_at(rate: Rational) -> ExportSettings {
        ExportSettings::new(640, 480, rate, sub_export::Container::Mkv)
    }

    #[test]
    fn the_canvas_and_timebase_come_from_the_sequence_not_the_preset() {
        let library = PresetLibrary::builtin();
        let (project, request) = project_at(Rational::FPS_24, 48);
        let settings = settings_for(&library, &project, &request).expect("the preset resolves");
        let preset = library.require(&request.preset.id).expect("the preset");
        // The compositor reads back at the sequence canvas and nothing
        // rescales a finished frame, so that is what is encoded.
        assert_eq!((settings.width, settings.height), (640, 480));
        assert_eq!(settings.frame_rate, Rational::FPS_24);
        assert_eq!(settings.container, preset.container);
        assert_eq!(
            settings.video_codec,
            preset.video.as_ref().expect("a video preset").codec
        );
    }

    #[test]
    fn the_gui_resolves_the_settings_the_cli_render_resolves() {
        let library = PresetLibrary::builtin();
        let (project, request) = project();
        let sequence = &project.sequences[0];
        let preset = library.require(&request.preset.id).expect("the preset");
        let (expected, _) =
            sub_export::settings_for_sequence(preset, sequence).expect("the preset resolves");
        let settings = settings_for(&library, &project, &request).expect("the preset resolves");
        assert_eq!(settings, expected);
    }

    #[test]
    fn a_plugin_preset_is_refused_rather_than_silently_substituted() {
        let library = PresetLibrary::builtin();
        let (project, mut request) = project();
        request.preset.source = PresetSource::Plugin {
            plugin: PluginId::parse("com.example.exporter").expect("a valid plugin id"),
        };
        let error =
            settings_for(&library, &project, &request).expect_err("a plugin preset is unsupported");
        assert_eq!(error.code, codes::EXPORT_PRESET_UNSUPPORTED);
        assert_eq!(
            error
                .details
                .get("preset")
                .and_then(serde_json::Value::as_str),
            Some(request.preset.id.as_str())
        );
    }

    #[test]
    fn an_unknown_preset_is_refused() {
        let library = PresetLibrary::builtin();
        let (project, mut request) = project();
        request.preset.id = "no-such-preset".to_owned();
        let error = settings_for(&library, &project, &request).expect_err("an unknown preset");
        assert_eq!(error.code, sub_export::codes::PRESET_UNKNOWN);
    }

    #[test]
    fn a_sequence_the_project_has_lost_is_refused() {
        let library = PresetLibrary::builtin();
        let (mut project, request) = project();
        project.sequences.clear();
        let error = settings_for(&library, &project, &request).expect_err("the sequence is gone");
        assert_eq!(error.code, sub_core::codes::NOT_FOUND);
    }

    #[test]
    fn an_uncatalogued_encoder_override_is_refused() {
        let library = PresetLibrary::builtin();
        let (project, mut request) = project();
        request.encoder_override = Some("notanencoder".to_owned());
        let settings = settings_for(&library, &project, &request).expect("the preset resolves");
        let error = elements_for(&settings, &request).expect_err("an unknown element");
        assert_eq!(error.code, sub_export::codes::UNKNOWN_ENCODER);
    }

    #[test]
    fn frame_count_is_the_span_itself_at_the_settings_own_rate() {
        let (_, request) = project_at(Rational::FPS_24, 48);
        assert_eq!(frames_total(&request, &settings_at(Rational::FPS_24)), 48);
    }

    #[test]
    fn frame_count_rescales_to_the_settings_rate_and_rounds_up() {
        let settings = settings_at(Rational::FPS_30);
        // One frame at 25 is a twenty-fifth of a second: 1.2 frames at 30, and
        // an export covers it rather than clipping it.
        let (_, request) = project_at(Rational::FPS_25, 1);
        assert_eq!(frames_total(&request, &settings), 2);
        // Five of them are exactly six frames at 30, with no rounding at all.
        let (_, request) = project_at(Rational::FPS_25, 5);
        assert_eq!(frames_total(&request, &settings), 6);
    }

    #[test]
    fn a_negative_frame_count_never_reaches_the_job() {
        let (_, mut request) = project();
        request.span = TimeRange::empty_at(RationalTime::from_frames(0, Rational::FPS_24));
        assert_eq!(frames_total(&request, &settings_at(Rational::FPS_24)), 0);
    }

    #[test]
    fn an_idle_runner_polls_and_cancels_without_a_job() {
        let mut runner = ExportRunner::new();
        let mut panel = crate::export_panel::ExportPanel::new();
        assert!(!runner.is_running());
        assert!(runner.handle().is_none());
        assert_eq!(runner.poll(&mut panel), 0);
        runner.cancel();
    }

    #[test]
    fn a_second_export_is_refused_while_one_runs() {
        let jobs = JobService::new(1);
        let library = PresetLibrary::builtin();
        let (project, request) = project();
        let mut runner = ExportRunner::new();
        // A job that never starts is still a job: the runner is busy from the
        // moment it is queued, whatever GStreamer makes of it.
        let mut sources = |_: &ExportRequest, settings: &sub_export::ExportSettings| {
            Ok(ExportStreams::video(Box::new(SolidFrames::new(
                settings.frame_bytes(),
                1,
            ))))
        };
        if runner
            .start(&jobs, &library, &project, &request, &mut sources)
            .is_err()
        {
            // No usable encoder on this machine; the refusal below is then
            // untestable and the environment, not the code, is at fault.
            return;
        }
        let error = runner
            .start(&jobs, &library, &project, &request, &mut sources)
            .expect_err("a second export is refused");
        assert_eq!(error.code, codes::EXPORT_BUSY);
        runner.cancel();
        jobs.wait_idle();
    }

    #[test]
    fn a_refused_start_leaves_the_runner_free() {
        let jobs = JobService::new(1);
        let library = PresetLibrary::builtin();
        let (project, mut request) = project();
        request.preset.id = "no-such-preset".to_owned();
        let mut runner = ExportRunner::new();
        let error = runner
            .start(
                &jobs,
                &library,
                &project,
                &request,
                &mut |_: &ExportRequest, settings: &sub_export::ExportSettings| {
                    Ok(ExportStreams::video(Box::new(SolidFrames::new(
                        settings.frame_bytes(),
                        1,
                    ))))
                },
            )
            .expect_err("an unknown preset");
        assert_eq!(error.code, sub_export::codes::PRESET_UNKNOWN);
        assert!(!runner.is_running());
    }

    #[test]
    fn a_source_that_refuses_stops_the_export_before_it_is_queued() {
        let jobs = JobService::new(1);
        let library = PresetLibrary::builtin();
        let (project, request) = project();
        let settings = settings_for(&library, &project, &request).expect("the preset resolves");
        if elements_for(&settings, &request).is_err() {
            // The elements are resolved before the sources are opened, so a
            // machine with no encoder never reaches the refusal under test.
            eprintln!("skipping: this machine has no usable encoder for the first built-in preset");
            return;
        }
        let mut runner = ExportRunner::new();
        let error = runner
            .start(
                &jobs,
                &library,
                &project,
                &request,
                &mut |_: &ExportRequest, _: &sub_export::ExportSettings| {
                    Err(sub_core::SubError::new(
                        sub_core::codes::NOT_FOUND,
                        "the media is offline",
                    ))
                },
            )
            .expect_err("the source refused");
        assert_eq!(error.code, sub_core::codes::NOT_FOUND);
        assert!(!runner.is_running());
    }

    #[test]
    fn streams_carry_audio_when_given_some() {
        let streams = ExportStreams::video(Box::new(SolidFrames::new(4, 1)));
        assert!(streams.audio.is_none());
        let streams = streams.with_audio(Box::new(sub_export::PcmAudioSource::new(vec![0.0; 8])));
        assert!(streams.audio.is_some());
        assert!(format!("{streams:?}").contains("audio: true"));
    }

    #[test]
    fn every_builtin_preset_names_a_codec_the_override_list_knows() {
        // The panel offers encoder names per codec; elements_for pins one of
        // them, so the two lists must be the same list.
        for preset in PresetLibrary::builtin().iter() {
            let Some(video) = preset.video.as_ref() else {
                continue;
            };
            let names = sub_export::encoder_names(video.codec);
            assert!(!names.is_empty(), "no encoders catalogued for {video:?}");
            assert!(matches!(
                video.codec,
                VideoCodec::H264 | VideoCodec::H265 | VideoCodec::Av1
            ));
        }
    }
}
