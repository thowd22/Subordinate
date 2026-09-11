//! The export panel: pick a preset, a sequence, a range and a file, then
//! watch the render (docs/PLAN.md §5.5, §5.7).
//!
//! The panel is a view over things other crates own. The preset list is a
//! [`PresetLibrary`] from `sub-export` folded together with the presets
//! exporter plugins contribute through the `exporter` world, so a plugin's
//! target sits in the same list as a built-in and is chosen the same way. The
//! progress bar, the ETA and the Cancel button are driven by the
//! [`ExportEvent`] stream of a running [`ExportJob`](sub_export::ExportJob):
//! the panel never renders anything itself, it folds events into a status and
//! paints it.
//!
//! Nothing here performs work. A click becomes an [`ExportAction`] the host
//! carries out — starting a job, cancelling one, choosing a file, revealing a
//! finished one — which keeps the panel free of threads and of the file
//! system, and makes the request it builds assertable in a test.
//!
//! Every time in a request is a [`RationalTime`] at the sequence's own rate:
//! the in and out points are frame numbers, and the range handed to the
//! exporter is a [`TimeRange`], so no float ever decides which frames are
//! written.
//!
//! ```
//! use sub_export::PresetLibrary;
//! use sub_model::sequence::SequenceSettings;
//! use sub_model::{Gap, Project, Sequence, Track, TrackItem, TrackKind};
//! use sub_time::RationalTime;
//! use sub_ui::export_panel::{ExportPanel, ExportRange};
//!
//! let mut project = Project::new("demo");
//! let mut sequence = Sequence::new("Main", SequenceSettings::default());
//! let rate = sequence.settings.frame_rate;
//! let mut track = Track::new("V1", TrackKind::Video);
//! track.items.push(TrackItem::Gap(Gap::new(RationalTime::from_frames(48, rate))));
//! sequence.tracks.push(track);
//! project.sequences.push(sequence);
//! let sequence = project.sequences[0].id;
//!
//! let mut panel = ExportPanel::new();
//! panel.set_library(&PresetLibrary::builtin());
//! panel.select_sequence(&project, sequence);
//! panel.set_output("/tmp/demo.mp4");
//!
//! let request = panel.request(&project).expect("the panel is ready");
//! assert_eq!(request.range, ExportRange::WholeSequence);
//! assert_eq!(request.preset.id, "youtube-1080p");
//! assert_eq!(request.frames_total(), 48);
//! ```

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{self, Ui};
use sub_core::{SubError, SubResult};
use sub_export::{
    EncoderStats, ExportEvent, ExportProgress, ExportReport, Preset, PresetLibrary, VideoCodec,
    encoder_names,
};
use sub_model::{Project, Sequence, SequenceId};
use sub_plugin::ExportPreset;
use sub_plugin::manifest::PluginId;
use sub_time::{Rational, RationalTime, TimeRange, Timecode, TimecodeRate};

use crate::codes;

/// The panel's title, in the dock tab and in its pop-out window.
pub const PANEL_TITLE: &str = "Export";

/// What the preset list shows when there is nothing in it at all.
pub const NO_PRESETS_LABEL: &str = "No export presets";

/// What the encoder picker shows for "whatever the plan picks".
pub const AUTOMATIC_ENCODER_LABEL: &str = "Automatic";

/// What the recent list shows before anything has been exported.
pub const NO_RECENT_LABEL: &str = "Nothing exported yet";

/// The label of the button that starts the export.
pub const START_LABEL: &str = "Export";

/// The label of the button that stops a running export.
pub const CANCEL_LABEL: &str = "Cancel";

/// The label of the button that reveals a finished file.
pub const OPEN_FOLDER_LABEL: &str = "Open folder";

/// How many finished exports the recent list remembers.
pub const RECENT_LIMIT: usize = 8;

/// Where a preset in the list came from.
///
/// The distinction matters when the request is performed: a library preset
/// carries [`ExportSettings`](sub_export::ExportSettings) the exporter can
/// build directly, while a plugin preset is a description the host hands back
/// to the plugin that offered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetSource {
    /// A built-in preset, or one from the user's `presets.toml`.
    Library,
    /// A preset an exporter plugin contributed.
    Plugin {
        /// The plugin that offered it.
        plugin: PluginId,
    },
}

impl PresetSource {
    /// The word the list shows beside a preset's name.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Library => "built in".to_owned(),
            Self::Plugin { plugin } => format!("plugin {plugin}"),
        }
    }
}

/// One row of the preset list, whatever offered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetEntry {
    /// The stable identifier a request carries.
    pub id: String,
    /// The name a user reads.
    pub name: String,
    /// A one-line description of what the file will be.
    pub summary: String,
    /// The video codec an encoder override would apply to, or `None` for a
    /// preset whose encoder the panel does not choose.
    pub codec: Option<VideoCodec>,
    /// Where the preset came from.
    pub source: PresetSource,
}

impl PresetEntry {
    /// The row for a library preset.
    #[must_use]
    pub fn from_preset(preset: &Preset) -> Self {
        Self {
            id: preset.id.clone(),
            name: preset.name.clone(),
            summary: preset_summary(preset),
            codec: preset.video.as_ref().map(|video| video.codec),
            source: PresetSource::Library,
        }
    }

    /// The row for a preset an exporter plugin contributed.
    #[must_use]
    pub fn from_plugin(plugin: &PluginId, preset: &ExportPreset) -> Self {
        let mut summary = preset.container.to_uppercase();
        if let Some(rate) = preset.frame_rate {
            let _ = write!(summary, " · {rate} fps");
        }
        if !preset.description.is_empty() {
            let _ = write!(summary, " · {}", preset.description);
        }
        Self {
            id: preset.id.clone(),
            name: preset.name.clone(),
            summary,
            codec: None,
            source: PresetSource::Plugin {
                plugin: plugin.clone(),
            },
        }
    }

    /// The line the picker shows: the name and where it came from.
    #[must_use]
    pub fn list_label(&self) -> String {
        format!("{} ({})", self.name, self.source.label())
    }
}

/// The one-line description of a library preset.
fn preset_summary(preset: &Preset) -> String {
    let container = preset.container.as_str().to_uppercase();
    let video = preset.video.as_ref().map_or_else(
        || "audio only".to_owned(),
        |video| {
            format!(
                "{}×{} · {} fps · {} · {}",
                video.width,
                video.height,
                video.frame_rate,
                video.codec.as_str(),
                video.quality
            )
        },
    );
    let audio = preset.audio.as_ref().map_or_else(
        || " · silent".to_owned(),
        |audio| format!(" · {}", audio.codec.as_str()),
    );
    format!("{container} · {video}{audio}")
}

/// Which part of the sequence an export covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportRange {
    /// Everything from the first frame to the end of the longest track.
    #[default]
    WholeSequence,
    /// The frames between the panel's in and out points.
    InToOut,
}

impl ExportRange {
    /// The label of the radio button that chooses it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::WholeSequence => "Whole sequence",
            Self::InToOut => "In to out",
        }
    }
}

/// Everything an export needs, as the panel has it.
///
/// This is what a click on Export produces and what a test asserts on: the
/// host turns it into an [`ExportJob`](sub_export::ExportJob) and never has to
/// re-read the panel's widgets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    /// The preset chosen, with its source so the host knows who resolves it.
    pub preset: PresetEntry,
    /// The sequence to render.
    pub sequence: SequenceId,
    /// Which part of it was asked for.
    pub range: ExportRange,
    /// The frames to render, at the sequence's own rate.
    pub span: TimeRange,
    /// The file to write.
    pub output: PathBuf,
    /// The encoder element the user pinned, or `None` for the plan's order.
    pub encoder_override: Option<String>,
}

impl ExportRequest {
    /// How many video frames the export will write.
    #[must_use]
    pub fn frames_total(&self) -> u64 {
        u64::try_from(self.span.duration().value()).unwrap_or(0)
    }
}

/// What a click on the panel asks the host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportAction {
    /// Start this export.
    Start(Box<ExportRequest>),
    /// Stop the running export at the next frame boundary.
    Cancel,
    /// Ask for an output file with the platform's save dialog.
    ChooseOutput,
    /// Show this file in the platform's file manager.
    Reveal(PathBuf),
}

/// How the export the panel is watching is going.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ExportStatus {
    /// Nothing is running and nothing has run in this session.
    #[default]
    Idle,
    /// An export is running; the snapshot is `None` until the first progress
    /// event arrives.
    Running {
        /// The file being written.
        path: PathBuf,
        /// Video frames expected, or `0` when unknown.
        frames_total: u64,
        /// The latest progress snapshot.
        progress: Option<Box<ExportProgress>>,
        /// What the elements are doing.
        stats: Box<EncoderStats>,
    },
    /// The last export wrote its file.
    Finished(Box<ExportReport>),
    /// The last export was cancelled.
    Cancelled {
        /// Frames written before the cancel was seen.
        frames_done: u64,
    },
    /// The last export failed.
    Failed {
        /// What went wrong; a pipeline failure names the element.
        error: Box<SubError>,
        /// Frames written before the failure.
        frames_done: u64,
    },
}

impl ExportStatus {
    /// Whether an export is running right now.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    /// The line under the progress bar.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Idle => "Idle".to_owned(),
            Self::Running {
                frames_total,
                progress,
                ..
            } => progress.as_ref().map_or_else(
                || format!("Starting: 0 of {frames_total} frames"),
                |progress| progress_line(progress),
            ),
            Self::Finished(report) => format!(
                "Wrote {} ({} frames, {})",
                report.path.display(),
                report.video_frames,
                report.video_encoder
            ),
            Self::Cancelled { frames_done } => {
                format!("Cancelled after {frames_done} frames; the part file was removed")
            }
            Self::Failed {
                error,
                frames_done: _,
            } => format!("Failed [{}]: {}", error.code, error.message),
        }
    }

    /// How far along the export is, as a fraction for the progress bar, or
    /// `None` when there is nothing to show.
    #[must_use]
    pub fn fraction(&self) -> Option<f32> {
        match self {
            Self::Running { progress, .. } => {
                let percent_milli = progress.as_ref()?.percent_milli?;
                Some(bar_fraction(percent_milli))
            }
            Self::Finished(_) => Some(1.0),
            Self::Idle | Self::Cancelled { .. } | Self::Failed { .. } => None,
        }
    }
}

/// A percentage in thousandths, as the 0..=1 fraction a progress bar wants.
///
/// Display only: the export's own arithmetic stays in integers, and this is
/// the one place the number becomes a width on screen.
#[allow(
    clippy::cast_precision_loss,
    reason = "a progress bar is pixels, and 100_000 is exact in f32"
)]
fn bar_fraction(percent_milli: u64) -> f32 {
    (percent_milli.min(100_000) as f32) / 100_000.0
}

/// The progress line: percent, frames, rate and ETA.
fn progress_line(progress: &ExportProgress) -> String {
    let percent = progress
        .percent()
        .map_or_else(String::new, |percent| format!("{percent}% · "));
    let rate = progress.frames_per_second_milli / 1_000;
    let eta = progress
        .eta
        .map_or_else(|| "unknown".to_owned(), duration_label);
    format!(
        "{percent}{} of {} frames · {rate} fps · {eta} left",
        progress.frames_done, progress.frames_total
    )
}

/// A wall-clock span as `h:mm:ss` or `m:ss`, in whole seconds.
///
/// Integer arithmetic throughout: an ETA that flickers between two renderings
/// of the same second reads as instability that is not there.
#[must_use]
pub fn duration_label(span: Duration) -> String {
    let total = span.as_secs();
    let (hours, minutes, seconds) = (total / 3_600, (total % 3_600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// One finished export, as the recent list remembers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentExport {
    /// The file that was written.
    pub path: PathBuf,
    /// The preset it was written with, by name.
    pub preset: String,
    /// Video frames written.
    pub frames: u64,
    /// The media duration written.
    pub duration: RationalTime,
    /// The video encoder element that ran.
    pub encoder: String,
}

impl RecentExport {
    /// The line the list shows.
    #[must_use]
    pub fn label(&self) -> String {
        let name = self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        format!("{name} — {} · {} frames", self.preset, self.frames)
    }
}

/// The export panel's state.
///
/// The panel owns only the choices: the project, the presets and the job all
/// belong to somebody else and are handed in.
#[derive(Debug, Clone, Default)]
pub struct ExportPanel {
    /// Every preset offered, library first then plugins, in the order they
    /// were added.
    presets: Vec<PresetEntry>,
    /// The id of the preset chosen, if the list has one.
    selected: Option<String>,
    /// The sequence to render, if one has been chosen.
    sequence: Option<SequenceId>,
    /// Which part of the sequence to render.
    range: ExportRange,
    /// The in point, as a frame number at the sequence rate.
    in_frame: i64,
    /// The out point, exclusive, as a frame number at the sequence rate.
    out_frame: i64,
    /// The file to write.
    output: PathBuf,
    /// The encoder element the user pinned for the selected preset's codec.
    encoder_override: Option<String>,
    /// How the export being watched is going.
    status: ExportStatus,
    /// The name of the preset the running export was started with, so a
    /// finished one can be remembered by it.
    running_preset: Option<String>,
    /// Finished exports, newest first.
    recent: Vec<RecentExport>,
    /// Preset load failures worth showing rather than logging.
    problems: Vec<SubError>,
    /// Set when the Choose… button is pressed, because the row that draws it
    /// already borrows the panel and cannot return an action of its own.
    output_pressed: Option<ExportAction>,
    /// Why this build cannot start an export, when it cannot. The panel still
    /// shows every choice and still builds a request; only the button that
    /// would hand it to a renderer is held closed.
    unavailable: Option<String>,
}

impl ExportPanel {
    /// An empty panel: no presets, no sequence, nothing running.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the library presets, keeping any plugin presets already added.
    ///
    /// The first preset is selected when nothing was, so the panel is usable
    /// without a click.
    pub fn set_library(&mut self, library: &PresetLibrary) {
        self.presets
            .retain(|entry| entry.source != PresetSource::Library);
        let library: Vec<PresetEntry> = library.iter().map(PresetEntry::from_preset).collect();
        // Library presets come first: they are what most exports use.
        let plugins = std::mem::replace(&mut self.presets, library);
        self.presets.extend(plugins);
        self.settle_selection();
    }

    /// Adds everything one exporter plugin offers, replacing what that same
    /// plugin offered before.
    pub fn set_plugin_presets(&mut self, plugin: &PluginId, presets: &[ExportPreset]) {
        self.presets.retain(|entry| {
            !matches!(&entry.source, PresetSource::Plugin { plugin: owner } if owner == plugin)
        });
        self.presets.extend(
            presets
                .iter()
                .map(|preset| PresetEntry::from_plugin(plugin, preset)),
        );
        self.settle_selection();
    }

    /// Says that this build cannot start an export, and why, or clears the
    /// message with `None`.
    ///
    /// The host sets it when it has no renderer to hand a request to, so the
    /// panel explains the disabled button rather than failing on the click.
    pub fn set_unavailable(&mut self, reason: Option<impl Into<String>>) {
        self.unavailable = reason.map(Into::into);
    }

    /// Why an export cannot be started here, when it cannot.
    #[must_use]
    pub fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    /// Records a problem to show beside the list, such as a preset file that
    /// would not parse.
    pub fn add_problem(&mut self, problem: SubError) {
        self.problems.push(problem);
    }

    /// The problems shown beside the list.
    #[must_use]
    pub fn problems(&self) -> &[SubError] {
        &self.problems
    }

    /// Every preset offered, in list order.
    #[must_use]
    pub fn presets(&self) -> &[PresetEntry] {
        &self.presets
    }

    /// The preset chosen, if the list has one.
    #[must_use]
    pub fn selected_preset(&self) -> Option<&PresetEntry> {
        let id = self.selected.as_deref()?;
        self.presets.iter().find(|entry| entry.id == id)
    }

    /// Chooses the preset with this id.
    ///
    /// # Errors
    ///
    /// [`codes::EXPORT_NOT_READY`] with a `preset` detail when no preset in
    /// the list carries that id.
    pub fn select_preset(&mut self, id: &str) -> SubResult<()> {
        if !self.presets.iter().any(|entry| entry.id == id) {
            return Err(not_ready("preset", format!("no export preset '{id}'")));
        }
        if self.selected.as_deref() != Some(id) {
            // The override belongs to the codec of the preset it was chosen
            // for, so it does not survive a change of preset.
            self.encoder_override = None;
        }
        self.selected = Some(id.to_owned());
        Ok(())
    }

    /// The sequence chosen, if one has been.
    #[must_use]
    pub const fn sequence(&self) -> Option<SequenceId> {
        self.sequence
    }

    /// Chooses a sequence and resets the in and out points to cover it.
    ///
    /// An id the project does not have leaves the panel alone, which is what
    /// a closed sequence should do.
    pub fn select_sequence(&mut self, project: &Project, id: SequenceId) {
        let Some(sequence) = project.sequences.iter().find(|other| other.id == id) else {
            return;
        };
        self.sequence = Some(id);
        self.in_frame = 0;
        self.out_frame = sequence_frames(sequence);
    }

    /// Which part of the sequence is being exported.
    #[must_use]
    pub const fn range(&self) -> ExportRange {
        self.range
    }

    /// Chooses whole-sequence or in-to-out.
    pub const fn set_range(&mut self, range: ExportRange) {
        self.range = range;
    }

    /// The in and out points, as frame numbers at the sequence rate.
    #[must_use]
    pub const fn in_out(&self) -> (i64, i64) {
        (self.in_frame, self.out_frame)
    }

    /// Moves the in and out points, keeping them ordered and non-negative.
    pub const fn set_in_out(&mut self, in_frame: i64, out_frame: i64) {
        self.in_frame = if in_frame < 0 { 0 } else { in_frame };
        self.out_frame = if out_frame < self.in_frame {
            self.in_frame
        } else {
            out_frame
        };
    }

    /// The file the export will write.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Sets the file the export will write.
    pub fn set_output(&mut self, path: impl Into<PathBuf>) {
        self.output = path.into();
    }

    /// The encoder element pinned for the selected preset, if any.
    #[must_use]
    pub fn encoder_override(&self) -> Option<&str> {
        self.encoder_override.as_deref()
    }

    /// Pins an encoder element, or clears the pin with `None`.
    ///
    /// # Errors
    ///
    /// [`codes::EXPORT_NOT_READY`] with an `encoder` detail when the element
    /// is not one the selected preset's codec can be encoded with. Whether it
    /// is available on this machine is the probe's question, not the panel's.
    pub fn set_encoder_override(&mut self, element: Option<&str>) -> SubResult<()> {
        let Some(element) = element else {
            self.encoder_override = None;
            return Ok(());
        };
        if !self.encoder_choices().contains(&element) {
            return Err(not_ready(
                "encoder",
                format!("{element} is not an encoder the selected preset can use"),
            ));
        }
        self.encoder_override = Some(element.to_owned());
        Ok(())
    }

    /// The encoder elements the selected preset could be encoded with.
    ///
    /// Empty for a preset whose codec the panel does not know, which is every
    /// plugin preset: the plugin picks its own encoder.
    #[must_use]
    pub fn encoder_choices(&self) -> Vec<&'static str> {
        self.selected_preset()
            .and_then(|entry| entry.codec)
            .map(encoder_names)
            .unwrap_or_default()
    }

    /// How the export being watched is going.
    #[must_use]
    pub const fn status(&self) -> &ExportStatus {
        &self.status
    }

    /// The finished exports, newest first.
    #[must_use]
    pub fn recent(&self) -> &[RecentExport] {
        &self.recent
    }

    /// Folds one [`ExportEvent`] from the running job into the panel.
    ///
    /// This is the whole wiring between the job and the panel: the host drains
    /// [`ExportJobHandle::events`](sub_export::ExportJobHandle::events) each
    /// frame and hands every event here.
    pub fn apply_event(&mut self, event: &ExportEvent) {
        match event {
            ExportEvent::Started {
                path,
                frames_total,
                stats,
            } => {
                self.status = ExportStatus::Running {
                    path: path.clone(),
                    frames_total: *frames_total,
                    progress: None,
                    stats: Box::new(stats.clone()),
                };
            }
            ExportEvent::Progress(progress) => {
                if let ExportStatus::Running {
                    progress: slot,
                    stats,
                    frames_total,
                    ..
                } = &mut self.status
                {
                    *frames_total = progress.frames_total;
                    **stats = progress.stats.clone();
                    *slot = Some(Box::new(progress.clone()));
                }
            }
            ExportEvent::Finished(report) => {
                self.remember(report);
                self.status = ExportStatus::Finished(Box::new(report.clone()));
            }
            ExportEvent::Cancelled { frames_done, .. } => {
                self.running_preset = None;
                self.status = ExportStatus::Cancelled {
                    frames_done: *frames_done,
                };
            }
            ExportEvent::Failed {
                error, frames_done, ..
            } => {
                self.running_preset = None;
                self.status = ExportStatus::Failed {
                    error: Box::new(error.clone()),
                    frames_done: *frames_done,
                };
            }
        }
    }

    /// Adds a finished export to the recent list, newest first and without
    /// repeating a path.
    fn remember(&mut self, report: &ExportReport) {
        let preset = self
            .running_preset
            .take()
            .or_else(|| self.selected_preset().map(|entry| entry.name.clone()))
            .unwrap_or_default();
        self.recent.retain(|entry| entry.path != report.path);
        self.recent.insert(
            0,
            RecentExport {
                path: report.path.clone(),
                preset,
                frames: report.video_frames,
                duration: report.duration,
                encoder: report.video_encoder.clone(),
            },
        );
        self.recent.truncate(RECENT_LIMIT);
    }

    /// The request the panel's current choices describe.
    ///
    /// # Errors
    ///
    /// [`codes::EXPORT_NOT_READY`], with a `field` detail naming what is
    /// missing: no preset chosen, no sequence chosen, no output file, a
    /// sequence the project has lost, or a range with no frames in it.
    pub fn request(&self, project: &Project) -> SubResult<ExportRequest> {
        let preset = self
            .selected_preset()
            .ok_or_else(|| not_ready("preset", "choose an export preset"))?
            .clone();
        let id = self
            .sequence
            .ok_or_else(|| not_ready("sequence", "choose a sequence to export"))?;
        let sequence = project
            .sequences
            .iter()
            .find(|sequence| sequence.id == id)
            .ok_or_else(|| not_ready("sequence", "the chosen sequence is no longer open"))?;
        if self.output.as_os_str().is_empty() {
            return Err(not_ready("output", "choose a file to write"));
        }
        let span = self.span(sequence)?;
        Ok(ExportRequest {
            preset,
            sequence: id,
            range: self.range,
            span,
            output: self.output.clone(),
            encoder_override: self.encoder_override.clone(),
        })
    }

    /// The frames the current range covers in `sequence`.
    fn span(&self, sequence: &Sequence) -> SubResult<TimeRange> {
        let rate = sequence.settings.frame_rate;
        let (start, end) = match self.range {
            ExportRange::WholeSequence => (0, sequence_frames(sequence)),
            ExportRange::InToOut => (self.in_frame, self.out_frame),
        };
        let span = TimeRange::from_start_end(
            RationalTime::from_frames(start, rate),
            RationalTime::from_frames(end, rate),
        )
        .ok_or_else(|| not_ready("range", "the out point must not precede the in point"))?;
        if span.is_empty() {
            return Err(not_ready("range", "the range has no frames in it"));
        }
        Ok(span)
    }

    /// Draws the panel and returns what the user asked for, if anything.
    ///
    /// `project` is the frame's immutable snapshot; the panel reads the
    /// sequence list from it and never changes it.
    pub fn ui(&mut self, ui: &mut Ui, project: &Project) -> Option<ExportAction> {
        let mut action = None;
        egui::ScrollArea::vertical()
            .id_salt("export_panel")
            .show(ui, |ui| {
                self.settings_ui(ui, project);
                ui.separator();
                action = self.progress_ui(ui, project);
                ui.separator();
                if let Some(reveal) = self.recent_ui(ui) {
                    action = Some(reveal);
                }
                if let Some(chosen) = self.output_pressed.take() {
                    // `output_pressed` is set by `settings_ui`, which cannot
                    // return an action of its own without borrowing twice.
                    action = Some(chosen);
                }
            });
        action
    }

    /// The preset, sequence, range, file and encoder rows.
    fn settings_ui(&mut self, ui: &mut Ui, project: &Project) {
        ui.heading(PANEL_TITLE);
        self.preset_ui(ui);
        self.sequence_ui(ui, project);
        self.range_ui(ui, project);
        self.output_ui(ui);
        self.encoder_ui(ui);
        for problem in &self.problems {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("[{}] {}", problem.code, problem.message),
            );
        }
    }

    /// The preset picker and the summary of what it writes.
    fn preset_ui(&mut self, ui: &mut Ui) {
        if self.presets.is_empty() {
            ui.label(NO_PRESETS_LABEL);
            return;
        }
        let selected = self
            .selected_preset()
            .map_or_else(|| NO_PRESETS_LABEL.to_owned(), PresetEntry::list_label);
        let mut chosen: Option<String> = None;
        egui::ComboBox::from_label("Preset")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for entry in &self.presets {
                    let picked = self.selected.as_deref() == Some(entry.id.as_str());
                    if ui
                        .selectable_label(picked, entry.list_label())
                        .on_hover_text(&entry.summary)
                        .clicked()
                    {
                        chosen = Some(entry.id.clone());
                    }
                }
            });
        if let Some(id) = chosen {
            // The id came from the list, so this cannot fail.
            let _ = self.select_preset(&id);
        }
        if let Some(entry) = self.selected_preset() {
            ui.label(&entry.summary);
        }
    }

    /// The sequence picker.
    fn sequence_ui(&mut self, ui: &mut Ui, project: &Project) {
        let selected = self
            .sequence
            .and_then(|id| project.sequences.iter().find(|sequence| sequence.id == id))
            .map_or_else(
                || "Choose a sequence".to_owned(),
                |sequence| sequence.name.clone(),
            );
        let mut chosen = None;
        egui::ComboBox::from_label("Sequence")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for sequence in &project.sequences {
                    let picked = self.sequence == Some(sequence.id);
                    if ui.selectable_label(picked, &sequence.name).clicked() {
                        chosen = Some(sequence.id);
                    }
                }
            });
        if let Some(id) = chosen {
            self.select_sequence(project, id);
        }
    }

    /// The range radio buttons and the in and out points.
    fn range_ui(&mut self, ui: &mut Ui, project: &Project) {
        ui.horizontal(|ui| {
            for range in [ExportRange::WholeSequence, ExportRange::InToOut] {
                if ui
                    .selectable_label(self.range == range, range.label())
                    .clicked()
                {
                    self.range = range;
                }
            }
        });
        let sequence = self
            .sequence
            .and_then(|id| project.sequences.iter().find(|sequence| sequence.id == id));
        let Some(sequence) = sequence else {
            return;
        };
        let rate = sequence.settings.frame_rate;
        let last = sequence_frames(sequence);
        match self.range {
            ExportRange::WholeSequence => {
                ui.label(format!("{} frames · {}", last, frame_label(last, rate)));
            }
            ExportRange::InToOut => {
                let (mut in_frame, mut out_frame) = (self.in_frame, self.out_frame);
                ui.horizontal(|ui| {
                    ui.label("In");
                    ui.add(egui::DragValue::new(&mut in_frame).range(0..=last));
                    ui.label("Out");
                    ui.add(egui::DragValue::new(&mut out_frame).range(0..=last));
                });
                self.set_in_out(in_frame, out_frame);
                ui.label(format!(
                    "{} frames · {} to {}",
                    self.out_frame - self.in_frame,
                    frame_label(self.in_frame, rate),
                    frame_label(self.out_frame, rate)
                ));
            }
        }
    }

    /// The output path field and its file dialog button.
    fn output_ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("File");
            let mut text = self.output.display().to_string();
            if ui.text_edit_singleline(&mut text).changed() {
                self.output = PathBuf::from(text);
            }
            if ui.button("Choose…").clicked() {
                self.output_pressed = Some(ExportAction::ChooseOutput);
            }
        });
    }

    /// The encoder override picker, when the preset has a codec to override.
    fn encoder_ui(&mut self, ui: &mut Ui) {
        let choices = self.encoder_choices();
        if choices.is_empty() {
            return;
        }
        let selected = self
            .encoder_override
            .clone()
            .unwrap_or_else(|| AUTOMATIC_ENCODER_LABEL.to_owned());
        let mut chosen: Option<Option<String>> = None;
        egui::ComboBox::from_label("Encoder")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(self.encoder_override.is_none(), AUTOMATIC_ENCODER_LABEL)
                    .clicked()
                {
                    chosen = Some(None);
                }
                for name in choices {
                    let picked = self.encoder_override.as_deref() == Some(name);
                    if ui.selectable_label(picked, name).clicked() {
                        chosen = Some(Some(name.to_owned()));
                    }
                }
            });
        if let Some(choice) = chosen {
            // Every name came from the list this preset offers.
            let _ = self.set_encoder_override(choice.as_deref());
        }
    }

    /// The progress bar, the status line and the Export or Cancel button.
    fn progress_ui(&mut self, ui: &mut Ui, project: &Project) -> Option<ExportAction> {
        let mut action = None;
        if let Some(fraction) = self.status.fraction() {
            ui.add(egui::ProgressBar::new(fraction).show_percentage());
        }
        let line = self.status.summary();
        if matches!(self.status, ExportStatus::Failed { .. }) {
            ui.colored_label(ui.visuals().error_fg_color, line);
        } else {
            ui.label(line);
        }
        if self.status.is_running() {
            if ui.button(CANCEL_LABEL).clicked() {
                action = Some(ExportAction::Cancel);
            }
            return action;
        }
        if let Some(reason) = self.unavailable.clone() {
            ui.add_enabled(false, egui::Button::new(START_LABEL));
            ui.label(reason);
            return action;
        }
        match self.request(project) {
            Ok(request) => {
                if ui.button(START_LABEL).clicked() {
                    self.running_preset = Some(request.preset.name.clone());
                    action = Some(ExportAction::Start(Box::new(request)));
                }
            }
            Err(error) => {
                ui.add_enabled(false, egui::Button::new(START_LABEL));
                ui.label(error.message.clone());
            }
        }
        action
    }

    /// The recent export list and its open-folder buttons.
    fn recent_ui(&mut self, ui: &mut Ui) -> Option<ExportAction> {
        ui.label("Recent exports");
        if self.recent.is_empty() {
            ui.label(NO_RECENT_LABEL);
            return None;
        }
        let mut action = None;
        for entry in &self.recent {
            ui.horizontal(|ui| {
                ui.label(entry.label())
                    .on_hover_text(entry.path.display().to_string());
                if ui.button(OPEN_FOLDER_LABEL).clicked() {
                    action = Some(ExportAction::Reveal(entry.path.clone()));
                }
            });
        }
        action
    }

    /// Keeps the selection pointing at a preset that still exists.
    fn settle_selection(&mut self) {
        let still_there = self
            .selected
            .as_deref()
            .is_some_and(|id| self.presets.iter().any(|entry| entry.id == id));
        if !still_there {
            self.selected = self.presets.first().map(|entry| entry.id.clone());
            self.encoder_override = None;
        }
    }
}

/// Shows the directory holding `path` in the platform's file manager.
///
/// # Errors
///
/// [`codes::EXPORT_FOLDER_UNOPENABLE`] when the opener cannot be started,
/// which on a headless machine it usually cannot. Failing to reveal a file is
/// never worth more than a message: the path is in the row beside it.
pub fn reveal(path: &Path) -> SubResult<()> {
    let folder = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    let target = folder.unwrap_or(path);
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|err| {
            SubError::wrap(
                codes::EXPORT_FOLDER_UNOPENABLE,
                "the export folder could not be opened",
                &err,
            )
            .with_detail("path", target.display().to_string())
            .with_detail("opener", program)
        })
}

/// How many frames long `sequence` is: its longest track.
#[must_use]
pub fn sequence_frames(sequence: &Sequence) -> i64 {
    let rate = sequence.settings.frame_rate;
    sequence
        .tracks
        .iter()
        .map(|track| track.duration(rate).value())
        .max()
        .unwrap_or(0)
}

/// A frame number as timecode, or as a frame count at a rate timecode cannot
/// label.
fn frame_label(frame: i64, rate: Rational) -> String {
    TimecodeRate::new(rate, TimecodeRate::rate_drops_frames(rate)).map_or_else(
        |_| format!("frame {frame}"),
        |rate| Timecode::from_frame_number(frame, rate).to_string(),
    )
}

/// A "the panel cannot build a request yet" error naming the field at fault.
fn not_ready(field: &str, message: impl Into<String>) -> SubError {
    SubError::new(codes::EXPORT_NOT_READY, message).with_detail("field", field)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use sub_export::{EncoderStats, ExportEvent, ExportProgress, ExportReport, PresetLibrary};
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Project, Sequence, SequenceId, Track, TrackKind};
    use sub_plugin::ExportPreset;
    use sub_plugin::manifest::PluginId;
    use sub_time::{Rational, RationalTime, TimeRange};

    use super::{
        ExportPanel, ExportRange, ExportStatus, PresetEntry, PresetSource, RECENT_LIMIT,
        bar_fraction, duration_label,
    };

    /// A project with one sequence holding a track `frames` long.
    fn project(frames: i64) -> (Project, SequenceId) {
        let mut project = Project::new("export");
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        let mut track = Track::new("V1", TrackKind::Video);
        let rate = sequence.settings.frame_rate;
        track
            .items
            .push(sub_model::TrackItem::Gap(sub_model::Gap::new(
                RationalTime::from_frames(frames, rate),
            )));
        sequence.tracks.push(track);
        let id = sequence.id;
        project.sequences.push(sequence);
        (project, id)
    }

    /// A panel over that project, ready to export the whole of it.
    fn panel(project: &Project, sequence: SequenceId) -> ExportPanel {
        let mut panel = ExportPanel::new();
        panel.set_library(&PresetLibrary::builtin());
        panel.select_sequence(project, sequence);
        panel.set_output("/tmp/out.mp4");
        panel
    }

    /// One plugin preset, as the exporter world describes them.
    fn plugin_preset(id: &str) -> ExportPreset {
        ExportPreset {
            id: id.to_owned(),
            name: format!("Plugin {id}"),
            description: "a plugin target".to_owned(),
            container: "mov".to_owned(),
            frame_rate: Some(Rational::FPS_24),
            settings: std::collections::BTreeMap::new(),
        }
    }

    /// The elements a report and its events name.
    fn stats() -> EncoderStats {
        EncoderStats {
            video_encoder: "x264enc".to_owned(),
            audio_encoder: Some("avenc_aac".to_owned()),
            muxer: "mp4mux".to_owned(),
            video_frames: 0,
            audio_frames: 0,
            bytes_written: 0,
            encoded: RationalTime::from_frames(0, Rational::FPS_24),
        }
    }

    /// A report for `path`, as a finished export raises one.
    fn report(path: &str, frames: u64) -> ExportReport {
        ExportReport {
            path: PathBuf::from(path),
            video_frames: frames,
            audio_frames: frames * 2,
            duration: RationalTime::from_frames(
                i64::try_from(frames).expect("a small frame count"),
                Rational::FPS_24,
            ),
            video_encoder: "x264enc".to_owned(),
            audio_encoder: Some("avenc_aac".to_owned()),
            muxer: "mp4mux".to_owned(),
        }
    }

    #[test]
    fn the_whole_sequence_range_covers_every_frame_of_the_longest_track() {
        let (project, sequence) = project(120);
        let panel = panel(&project, sequence);
        let request = panel.request(&project).expect("the panel is ready");
        assert_eq!(request.range, ExportRange::WholeSequence);
        assert_eq!(request.frames_total(), 120);
        assert_eq!(
            request.span,
            TimeRange::new(
                RationalTime::from_frames(0, Rational::FPS_24),
                RationalTime::from_frames(120, Rational::FPS_24),
            )
            .expect("a valid range"),
            "the span is exact frames at the sequence rate"
        );
    }

    #[test]
    fn the_in_to_out_range_exports_only_the_frames_between_the_points() {
        let (project, sequence) = project(120);
        let mut panel = panel(&project, sequence);
        panel.set_range(ExportRange::InToOut);
        panel.set_in_out(24, 48);
        let request = panel.request(&project).expect("the panel is ready");
        assert_eq!(request.range, ExportRange::InToOut);
        assert_eq!(request.frames_total(), 24);
        assert_eq!(
            request.span.start(),
            RationalTime::from_frames(24, Rational::FPS_24)
        );
    }

    #[test]
    fn the_points_stay_ordered_and_non_negative_however_they_are_dragged() {
        let (project, sequence) = project(120);
        let mut panel = panel(&project, sequence);
        panel.set_in_out(-10, 30);
        assert_eq!(panel.in_out(), (0, 30), "an in point cannot precede zero");
        panel.set_in_out(40, 10);
        assert_eq!(
            panel.in_out(),
            (40, 40),
            "an out point cannot precede the in"
        );
    }

    #[test]
    fn an_empty_range_and_every_missing_choice_name_their_own_field() {
        let (project, sequence) = project(120);
        let mut bare = ExportPanel::new();
        assert_eq!(
            bare.request(&project).unwrap_err().details["field"],
            "preset",
            "no preset is the first thing missing"
        );
        bare.set_library(&PresetLibrary::builtin());
        assert_eq!(
            bare.request(&project).unwrap_err().details["field"],
            "sequence"
        );
        bare.select_sequence(&project, sequence);
        assert_eq!(
            bare.request(&project).unwrap_err().details["field"],
            "output"
        );
        bare.set_output("/tmp/out.mp4");
        bare.set_range(ExportRange::InToOut);
        bare.set_in_out(30, 30);
        let error = bare.request(&project).unwrap_err();
        assert_eq!(error.details["field"], "range");
        assert_eq!(error.code, crate::codes::EXPORT_NOT_READY);
    }

    #[test]
    fn a_sequence_the_project_has_lost_is_refused_rather_than_exported() {
        let (project, sequence) = project(120);
        let panel = panel(&project, sequence);
        let empty = Project::new("empty");
        assert_eq!(
            panel.request(&empty).unwrap_err().details["field"],
            "sequence"
        );
    }

    #[test]
    fn plugin_presets_join_the_list_after_the_library_and_replace_their_own() {
        let (project, sequence) = project(120);
        let mut panel = panel(&project, sequence);
        let library = panel.presets().len();
        let plugin = PluginId::parse("com.example.exporter").expect("a valid id");
        panel.set_plugin_presets(
            &plugin,
            &[plugin_preset("web.small"), plugin_preset("web.big")],
        );
        assert_eq!(panel.presets().len(), library + 2);
        let entry = panel
            .presets()
            .iter()
            .find(|entry| entry.id == "web.small")
            .expect("the plugin preset is listed");
        assert_eq!(
            entry.source,
            PresetSource::Plugin {
                plugin: plugin.clone()
            }
        );
        assert!(entry.summary.starts_with("MOV"), "{}", entry.summary);

        // Loading the plugin again replaces what it offered before rather
        // than listing it twice.
        panel.set_plugin_presets(&plugin, &[plugin_preset("web.small")]);
        assert_eq!(panel.presets().len(), library + 1);

        // A plugin preset can be chosen, and then there is no encoder to
        // override: the plugin picks its own.
        panel.select_preset("web.small").expect("it is listed");
        assert!(panel.encoder_choices().is_empty());
        let request = panel.request(&project).expect("the panel is ready");
        assert_eq!(request.preset.id, "web.small");
    }

    #[test]
    fn the_library_can_be_reloaded_without_losing_the_plugins_presets() {
        let (project, sequence) = project(120);
        let mut panel = panel(&project, sequence);
        let plugin = PluginId::parse("com.example.exporter").expect("a valid id");
        panel.set_plugin_presets(&plugin, &[plugin_preset("web.small")]);
        panel.set_library(&PresetLibrary::builtin());
        assert!(
            panel.presets().iter().any(|entry| entry.id == "web.small"),
            "reloading the library keeps what plugins contributed"
        );
        assert_eq!(
            panel.presets().first().map(|entry| entry.source.clone()),
            Some(PresetSource::Library),
            "library presets stay at the top of the list"
        );
    }

    #[test]
    fn an_encoder_override_belongs_to_the_preset_it_was_chosen_for() {
        let (project, sequence) = project(120);
        let mut panel = panel(&project, sequence);
        let element = *panel
            .encoder_choices()
            .first()
            .expect("h264 has catalogued encoders");
        panel
            .set_encoder_override(Some(element))
            .expect("it is listed");
        assert_eq!(
            panel.request(&project).expect("ready").encoder_override,
            Some(element.to_owned())
        );
        assert!(
            panel.set_encoder_override(Some("not-an-encoder")).is_err(),
            "an element the preset's codec cannot use is refused"
        );

        let other = panel.presets()[1].id.clone();
        panel.select_preset(&other).expect("it is listed");
        assert_eq!(
            panel.encoder_override(),
            None,
            "the pin does not survive a change of preset"
        );
    }

    #[test]
    fn an_unknown_preset_id_is_refused_and_leaves_the_choice_alone() {
        let (project, sequence) = project(120);
        let mut panel = panel(&project, sequence);
        let chosen = panel.selected_preset().map(|entry| entry.id.clone());
        assert!(panel.select_preset("no-such-preset").is_err());
        assert_eq!(
            panel.selected_preset().map(|entry| entry.id.clone()),
            chosen
        );
    }

    #[test]
    fn the_job_events_drive_the_progress_bar_the_eta_and_the_recent_list() {
        let (project, sequence) = project(120);
        let mut panel = panel(&project, sequence);
        assert_eq!(panel.status(), &ExportStatus::Idle);

        panel.apply_event(&ExportEvent::Started {
            path: PathBuf::from("/tmp/out.mp4"),
            frames_total: 120,
            stats: stats(),
        });
        assert!(panel.status().is_running());
        assert_eq!(panel.status().fraction(), None, "no snapshot, no bar");

        panel.apply_event(&ExportEvent::Progress(ExportProgress {
            frames_done: 30,
            frames_total: 120,
            elapsed: Duration::from_secs(2),
            eta: Some(Duration::from_secs(6)),
            frames_per_second_milli: 15_000,
            percent_milli: Some(25_000),
            stats: stats(),
        }));
        assert_eq!(panel.status().fraction(), Some(0.25));
        let line = panel.status().summary();
        assert!(line.contains("25%"), "{line}");
        assert!(line.contains("0:06 left"), "{line}");

        panel.apply_event(&ExportEvent::Finished(report("/tmp/out.mp4", 120)));
        assert!(!panel.status().is_running());
        assert_eq!(panel.status().fraction(), Some(1.0));
        assert_eq!(panel.recent().len(), 1);
        assert_eq!(panel.recent()[0].frames, 120);
        assert!(panel.recent()[0].label().starts_with("out.mp4"));
    }

    #[test]
    fn a_cancelled_and_a_failed_export_say_so_and_add_nothing_to_the_list() {
        let (project, sequence) = project(120);
        let mut panel = panel(&project, sequence);
        panel.apply_event(&ExportEvent::Cancelled {
            frames_done: 12,
            file_removed: true,
        });
        assert!(
            panel
                .status()
                .summary()
                .contains("Cancelled after 12 frames")
        );

        let error = sub_core::SubError::new(
            sub_export::codes::PIPELINE_FAILED,
            "x264enc refused the buffer",
        );
        panel.apply_event(&ExportEvent::Failed {
            error,
            frames_done: 3,
            file_removed: true,
        });
        let line = panel.status().summary();
        assert!(line.contains("export.pipeline_failed"), "{line}");
        assert!(panel.recent().is_empty(), "nothing was written to remember");
    }

    #[test]
    fn the_recent_list_is_newest_first_without_repeating_a_path() {
        let (project, sequence) = project(120);
        let mut panel = panel(&project, sequence);
        for index in 0..=RECENT_LIMIT {
            panel.apply_event(&ExportEvent::Finished(report(
                &format!("/tmp/out{index}.mp4"),
                u64::try_from(index).expect("a small index"),
            )));
        }
        assert_eq!(panel.recent().len(), RECENT_LIMIT, "the list is bounded");
        assert_eq!(
            panel.recent()[0].path,
            PathBuf::from(format!("/tmp/out{RECENT_LIMIT}.mp4")),
            "newest first"
        );

        panel.apply_event(&ExportEvent::Finished(report("/tmp/out0.mp4", 1)));
        let repeats = panel
            .recent()
            .iter()
            .filter(|entry| entry.path == Path::new("/tmp/out0.mp4"))
            .count();
        assert_eq!(
            repeats, 1,
            "an export to the same file moves rather than repeats"
        );
    }

    #[test]
    fn a_preset_entry_reads_as_the_file_it_writes() {
        let library = PresetLibrary::builtin();
        let preset = library.require("youtube-1080p").expect("a shipped preset");
        let entry = PresetEntry::from_preset(preset);
        assert!(
            entry.summary.starts_with("MP4 · 1920×1080"),
            "{}",
            entry.summary
        );
        assert!(
            entry.list_label().ends_with("(built in)"),
            "{}",
            entry.list_label()
        );
    }

    #[test]
    fn an_eta_reads_as_a_clock_and_a_percentage_as_a_bar_width() {
        assert_eq!(duration_label(Duration::from_secs(9)), "0:09");
        assert_eq!(duration_label(Duration::from_secs(125)), "2:05");
        assert_eq!(duration_label(Duration::from_secs(3_725)), "1:02:05");
        assert!((bar_fraction(0) - 0.0).abs() < f32::EPSILON);
        assert!((bar_fraction(50_000) - 0.5).abs() < f32::EPSILON);
        assert!(
            (bar_fraction(200_000) - 1.0).abs() < f32::EPSILON,
            "a percentage over 100 is still a full bar"
        );
    }

    #[test]
    fn an_unavailable_renderer_is_explained_rather_than_hidden() {
        let mut panel = ExportPanel::new();
        assert_eq!(panel.unavailable(), None);
        panel.set_unavailable(Some("no renderer here"));
        assert_eq!(panel.unavailable(), Some("no renderer here"));
        panel.set_unavailable(None::<String>);
        assert_eq!(panel.unavailable(), None);
    }
}
