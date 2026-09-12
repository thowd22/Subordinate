//! The eframe application, its wgpu setup, and the glue between the panels
//! and the engine.
//!
//! eframe runs on the wgpu backend so the device it creates for egui is the
//! very device the compositor draws preview frames with (docs/PLAN.md §3).
//! [`SubordinateApp::new`] lifts that device, queue and adapter out of
//! eframe's `RenderState` into a [`RenderContext`], which is what every other
//! crate sees.
//!
//! The window owns no project. It owns an [`EditorSession`]: the engine thread
//! that does (docs/PLAN.md §4), the immutable snapshot the panels read, and the
//! autosave worker watching it. Every panel here plans its gesture and hands
//! the plan back; this module applies each plan as one entry in the engine's
//! undo stack, on the same queue the Command API, the MCP bridge and plugins
//! use. The traffic runs both ways: the session's change-event subscription is
//! drained once a frame, so an edit made from outside the window appears in the
//! panels on the next one.

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use sub_core::{SubError, SubResult};
use sub_edit::commands::DeleteSequence;
use sub_edit::playback::{PlaybackScheduler, ShuttleSpeed};
use sub_model::sequence::{Resolution, Sequence, SequenceSettings};
use sub_model::{BinId, Project, SequenceId};
use sub_render::{
    Compositor, RenderContext, RenderError, ResolvedClip, SourceFrame, describe_adapter,
    select_adapter,
};
use sub_time::RationalTime;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sub_audio::mixer::{MixGraphBuilder, MixerConfig, MixerControl, mixer};
use sub_audio::scrub::{ScrubControl, ScrubSettings, scrub};
use sub_audio::{AudioOutput, CpalBackend, MeterBank, OutputOptions};
use sub_command::endpoint::{Address, DEFAULT_INSTANCE, Endpoint};
use sub_core::JobService;
use sub_export::PresetLibrary;

use crate::audio_settings::{AudioSettingsAction, AudioSettingsPanel};
use crate::command_api::CommandApi;
use crate::diagnostics::DiagnosticsPanel;
use crate::dock::{DockLayout, Panel, layout_menu_ui};
use crate::effects::EffectCatalog;
use crate::export_panel::{ExportAction, ExportPanel, ExportRequest};
use crate::export_runner::ExportRunner;
use crate::fullscreen::{FullscreenAction, FullscreenState, monitor_picker_ui};
use crate::history_panel::{HistoryAction, HistoryList, edit_menu_ui};
use crate::inspector::{EffectEdit, InspectorPanel, InspectorResponse};
use crate::keymap::LoadedKeymap;
use crate::media_bin::{BinSelection, BinStatus, MediaBinAction, MediaBinPanel};
use crate::media_import::{FinishedImport, ImportOutcome, ImportQueue};
use crate::popout::{PopoutViewer, popout_menu_ui};
use crate::preview::PreviewService;
use crate::recovery::{RecoveryOutcome, RecoveryPrompt, SnapshotMenu};
use crate::relink_dialog::RelinkDialog;
use crate::sequence_tabs::{SequenceTabAction, SequenceTabs, SequenceViewState};
use crate::session::EditorSession;
use crate::shortcuts::{Action, ShortcutMap, ShortcutsWindow};
use crate::source_edit::EditMode;
use crate::timeline_panel::{TimelinePanel, TimelineResponse, Tool};
use crate::viewer::{TransportAction, ViewerAction, ViewerFrame, ViewerPanel, sequence_duration};

/// How many tracks the shared meter bank has room for. A sequence with more
/// audio tracks than this still plays; the tracks past it are unmetered.
const METERED_TRACKS: usize = 64;

/// Worker threads the window's job pool runs. Imports, hashes, waveforms and
/// exports share them; two is enough that a long export does not shut the
/// interactive work out.
const JOB_WORKERS: usize = 2;

/// How many failures the media bin keeps on screen at once.
///
/// Enough that dropping a folder of mixed files shows what was refused,
/// few enough that the panel is still a bin rather than a log.
const MAX_BIN_PROBLEMS: usize = 8;

/// How often the window repaints itself while an export runs, so the progress
/// bar and the ETA keep moving without an input event to wake egui.
const EXPORT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the window waits before looking at the preview decoders again.
///
/// A decode worker landing a picture between paints cannot wake egui, so a
/// preview that is still catching up asks for the next frame itself. Short
/// enough that a scrub feels immediate, long enough that a parked playhead over
/// a slow file is not a spin.
const PREVIEW_POLL_INTERVAL: Duration = Duration::from_millis(8);

/// The File menu's open entry.
pub const OPEN_LABEL: &str = "Open project...";

/// The File menu's save entry.
pub const SAVE_LABEL: &str = "Save project";

/// The File menu's save-as entry.
pub const SAVE_AS_LABEL: &str = "Save project as...";

/// The extension a Subordinate project file has.
pub const PROJECT_EXTENSION: &str = "sub";

/// The undo entry one Import gesture leaves behind.
///
/// One gesture is one entry however many files it carried, so importing a
/// card of twenty rushes is undone with one press (TASK-136).
pub const IMPORT_GROUP_LABEL: &str = "Import media";

/// Why an import into a project that has never been saved is refused.
///
/// Media paths are stored relative to the project file, so a project with no
/// file has nothing for an imported path to be relative to. This is the same
/// rule that holds the Export button closed, said for import.
pub const NO_IMPORT_REASON: &str =
    "Save the project to a file before importing: its media is named relative to it";

/// What one frame's panels asked the engine to do.
///
/// The dock hands each panel a mutable borrow of itself, so nothing inside it
/// can reach the session; a frame's gestures are collected here and applied
/// once the dock has finished. That is also what keeps a gesture atomic: the
/// whole of it is applied in one place, as one entry in the undo stack.
#[derive(Default)]
struct FrameEdits {
    /// The media bin's actions, in the order they were raised.
    bin: Vec<MediaBinAction>,
    /// The timeline frame, when the timeline was drawn.
    timeline: Option<TimelineResponse>,
    /// The inspector's gesture, when it asked for one.
    inspector: Option<InspectorResponse>,
    /// What the export panel asked for, when it asked for anything.
    export: Option<ExportAction>,
}

/// The environment variable naming the Command API instance this editor
/// serves. `subordinate-mcp` reads the same one from the other side.
pub const INSTANCE_ENV: &str = "SUBORDINATE_INSTANCE";

/// The environment variable overriding where this editor's socket and lock
/// file live. `subordinate-mcp` reads the same one from the other side.
pub const ENDPOINT_DIR_ENV: &str = "SUBORDINATE_ENDPOINT_DIR";

/// The environment variable that stops the editor serving the Command API
/// (`1`, `true`, `yes`, `on`).
pub const NO_COMMAND_API_ENV: &str = "SUBORDINATE_NO_COMMAND_API";

/// Whether an environment variable spells "yes".
fn is_yes(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// The line the window smoke run prints once every window has a picture.
///
/// CI waits for it before it takes a screenshot, so it is part of the
/// contract with `scripts/ui-smoke.sh` (TASK-123) rather than a stray log
/// line. Anything after the colon is diagnostics.
pub const UI_SMOKE_READY: &str = "ui-smoke ready";

/// Why the export panel's Export button is held closed for an unsaved project.
///
/// Everything else an export needs is here: the panel, the job, and the
/// compositor readback and offline mix behind [`export_sources`]. What a
/// project that has never been written to a file lacks is a folder for its
/// media paths to resolve against — clips name their media relative to the
/// project file — so there is nothing for a decoder to open.
pub const NO_RENDERER_REASON: &str =
    "Save the project to a file before exporting: its media is named relative to it";

/// Options for launching the application.
#[derive(Debug, Clone, Default)]
pub struct AppOptions {
    /// Close the window after this many painted frames.
    ///
    /// `None` runs normally. `Some(n)` is the CI smoke test: the app starts,
    /// paints an empty window `n` times on whatever adapter the machine has
    /// (a software one on a hosted runner) and exits.
    pub smoke_frames: Option<u32>,
    /// A project to open as soon as the window is up.
    pub project: Option<PathBuf>,
    /// Pop the viewer out at startup, as the View menu would.
    pub open_popout: bool,
    /// Where to put the pop-out window, in points on the virtual desktop.
    ///
    /// This is how CI lands it on a second monitor with nobody there to drag
    /// it; a normal run leaves it to the window manager.
    pub popout_position: Option<[f32; 2]>,
    /// Close the window once it has been up this long.
    ///
    /// `None` runs until the user closes it. This is the window smoke run's
    /// self-destruct: it keeps the windows on screen long enough to be
    /// photographed and guarantees the process ends even if CI's capture step
    /// never gets that far.
    pub hold: Option<Duration>,
    /// Whether the window serves the Command API on a local socket, so the
    /// MCP bridge, the CLI and plugins drive *this* editor
    /// (see [`crate::command_api`]).
    ///
    /// The editor binary turns it on ([`AppOptions::from_env`]); it is off by
    /// default so that an app embedded in a test does not reach for the
    /// per-user endpoint a real editor may be holding.
    pub serve_command_api: bool,
    /// The instance name the endpoint is derived from. `None` is
    /// `sub_command::endpoint::DEFAULT_INSTANCE`, which is the one every
    /// client looks for first.
    pub instance: Option<String>,
    /// Where the socket and the lock file live, overriding the per-user
    /// runtime directory. Tests set it; a user has no reason to.
    pub endpoint_dir: Option<PathBuf>,
}

impl AppOptions {
    /// Read the smoke-frame count from `SUB_SMOKE_FRAMES`.
    ///
    /// An unset or unparsable value means "run normally".
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            smoke_frames: std::env::var("SUB_SMOKE_FRAMES")
                .ok()
                .and_then(|value| value.trim().parse().ok()),
            // The editor is what an agent drives, so a run started from the
            // command line serves its endpoint unless it is told not to. The
            // three variables are the ones `subordinate-mcp` reads from the
            // other side, so pointing a bridge and an editor at the same
            // private endpoint is one pair of settings, not two.
            serve_command_api: !std::env::var(NO_COMMAND_API_ENV).is_ok_and(|value| is_yes(&value)),
            instance: std::env::var(INSTANCE_ENV)
                .ok()
                .filter(|value| !value.is_empty()),
            endpoint_dir: std::env::var_os(ENDPOINT_DIR_ENV)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            ..Self::default()
        }
    }

    /// The endpoint these options name, when one is to be served.
    ///
    /// # Errors
    ///
    /// `command.invalid_instance` for an instance name that cannot go in a
    /// socket path or a pipe name, and `command.endpoint_unavailable` when
    /// this machine offers no per-user runtime directory.
    pub fn endpoint(&self) -> SubResult<Endpoint> {
        let instance = self.instance.as_deref().unwrap_or(DEFAULT_INSTANCE);
        match &self.endpoint_dir {
            Some(directory) => Endpoint::in_directory(directory.clone(), instance),
            None => Endpoint::for_instance(instance),
        }
    }

    /// Whether this run paints on its own rather than waiting for input.
    ///
    /// Both CI runs do: one counts frames, the other holds the window open
    /// for a wall-clock span. Nothing on screen animates, so egui has to be
    /// asked for the next frame either way.
    #[must_use]
    pub const fn is_unattended(&self) -> bool {
        self.smoke_frames.is_some() || self.hold.is_some()
    }
}

/// What became of the project named on the command line.
///
/// It reaches CI through the ready line: a smoke run that photographed an
/// empty editor because the project would not load is a failure, and this is
/// what makes that visible without reading the whole log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectState {
    /// No project was asked for.
    None,
    /// The project opened.
    Loaded,
    /// The project was asked for and would not open.
    Failed,
}

impl ProjectState {
    /// The word the ready line reports.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Loaded => "loaded",
            Self::Failed => "failed",
        }
    }
}

/// The import and relink side of the window.
///
/// Both are jobs rather than commands: importing has to hash and probe the
/// files before there is a [`MediaItem`](sub_model::MediaItem) to add, and
/// relinking has to find the replacement before there is a path to point at.
/// Neither may touch the UI thread, so both run on the window's
/// [`JobService`] and are pumped once a frame from
/// [`SubordinateApp::poll_media`]; what they produce is applied through the
/// session like every other edit, one history group per gesture (TASK-136).
struct MediaHost {
    /// The imports and their follow-up thumbnail and waveform jobs.
    ///
    /// `None` until the project has a file, because an imported path is
    /// stored relative to that file and a sidecar folder is named after it.
    /// Rebuilt whenever the project folder changes, so a Save As moves later
    /// imports with it.
    queue: Option<ImportQueue>,
    /// The project folder `queue` was built for, so the rebuild happens
    /// exactly when the folder moves.
    dir: Option<PathBuf>,
    /// The relink dialog, open for one item or for every offline one.
    relink: RelinkDialog,
    /// What the last import or relink got wrong, newest last. Shown in the
    /// bin, not logged and forgotten.
    problems: Vec<SubError>,
}

impl MediaHost {
    /// The media side of a fresh window: no queue, no dialog, nothing wrong.
    fn new() -> Self {
        Self {
            queue: None,
            dir: None,
            relink: RelinkDialog::new(),
            problems: Vec::new(),
        }
    }

    /// Records `error` for the bin to show, keeping the list short enough to
    /// draw.
    fn problem(&mut self, error: SubError) {
        log::warn!("[{}] {}", error.code, error.message);
        if self.problems.len() >= MAX_BIN_PROBLEMS {
            self.problems.remove(0);
        }
        self.problems.push(error);
    }

    /// What the bin should draw this frame.
    fn status(&self) -> BinStatus {
        BinStatus {
            importing: self
                .queue
                .as_ref()
                .map(|queue| {
                    queue
                        .pending_paths()
                        .into_iter()
                        .map(Path::to_path_buf)
                        .collect()
                })
                .unwrap_or_default(),
            problems: self.problems.clone(),
        }
    }
}

/// The Subordinate editor window.
pub struct SubordinateApp {
    render: RenderContext,
    /// eframe's own render state, kept for its egui renderer: registering the
    /// compositor output as an egui texture goes through it.
    render_state: RenderState,
    options: AppOptions,
    frames_painted: u32,
    closing: bool,
    /// When the app was built, which is what [`AppOptions::hold`] counts from.
    started: Instant,
    /// How the project named on the command line loaded, for the ready line.
    project_state: ProjectState,
    /// Whether the ready line has already been printed.
    announced_ready: bool,
    diagnostics: DiagnosticsPanel,
    /// The audio settings panel: which device plays, and how it is doing.
    audio_settings: AudioSettingsPanel,
    /// The output stage. It is closed until something plays; selecting a
    /// device while it is closed only records the choice, and selecting one
    /// while it is open reopens the stream there.
    audio: AudioOutput<CpalBackend>,
    /// Where the mixer publishes its levels. Shared with whatever mixer the
    /// output stage builds, so reopening a stream keeps the meters live.
    meters: Arc<MeterBank>,
    /// The sequence being previewed. Loading a project replaces it; until
    /// then it is an empty sequence, which composites to black.
    sequence: Sequence,
    /// The compositor drawing that sequence at the playhead.
    compositor: Compositor,
    /// The viewer panel: picture, scrub bar and timecode.
    viewer: ViewerPanel,
    /// The viewer's pop-out window, for a preview on a second display. It
    /// paints the same compositor texture the docked panel does.
    popout: PopoutViewer,
    /// The video scheduler behind J, K, L and the space bar. It owns the
    /// playhead while playback runs; the viewer owns it the rest of the time,
    /// and the two are synchronised once a frame. It measures no time itself:
    /// it follows the audio clock while the output stream is playing, and the
    /// monotonic fallback master otherwise (docs/PLAN.md §5.4).
    scheduler: PlaybackScheduler,
    /// The control half of whatever mixer the output stage last built, so the
    /// audio transport can be seeked to the playhead. `None` until a stream
    /// has been opened.
    audio_control: Rc<RefCell<Option<MixerControl>>>,
    /// The engine half of the scrub player attached to that mixer, so a drag
    /// on the playhead can ask for a grain (docs/PLAN.md §5.4). `None` until a
    /// stream has been opened.
    scrub_control: Rc<RefCell<Option<ScrubControl>>>,
    /// What scrubbing does, as the settings panel last left it. Shared with
    /// the mixer factory so a stream opened later starts with the same
    /// settings.
    scrub_settings: Rc<Cell<ScrubSettings>>,
    /// When the grain asked for last stops sounding. The output stage is held
    /// open until then, so a grain is not cut off by the stream closing the
    /// moment the drag pauses.
    scrub_until: Option<Instant>,
    /// The engine the whole editor edits through, the project it owns and the
    /// autosave worker watching it. No panel here holds a mutable project:
    /// they plan gestures, and this applies them as commands.
    session: EditorSession,
    /// The revision and active sequence [`SubordinateApp::sequence`] was taken
    /// at, so the clone is refreshed exactly when the engine has moved on or
    /// the user has changed tabs.
    synced: Option<(u64, Option<SequenceId>)>,
    /// The sequence tab strip, which owns which sequence is on screen and
    /// where each one was last being looked at.
    tabs: SequenceTabs,
    /// The media bin panel.
    media_bin: MediaBinPanel,
    /// The timeline panel.
    timeline: TimelinePanel,
    /// The inspector panel: the parameters of whatever the timeline has
    /// selected.
    inspector: InspectorPanel,
    /// The export side of the window: the panel, the presets it offers and
    /// the job it is running.
    export: ExportHost,
    /// The import and relink side of the window.
    media: MediaHost,
    /// The worker pool every file read in this window runs on. Imports,
    /// hashes, thumbnails, waveforms, relink searches and exports share it,
    /// which is what keeps a long export from shutting importing out.
    jobs: JobService,

    /// The effect plugins the inspector offers, and what each of them
    /// declared.
    ///
    /// Empty until this window hosts a plugin runtime: the editor scans and
    /// loads plugins through `sub-plugin`, which the app does not own yet, so
    /// the picker lists what the host has told it about and nothing else.
    effect_catalog: EffectCatalog,
    /// Where the panels are docked, as read from the user's `layout.json`.
    layout: DockLayout,
    /// Which display the pop-out goes fullscreen on, as read from the user's
    /// `fullscreen.json` and written back when it changes.
    fullscreen: FullscreenState,
    /// The decoded pictures behind the viewer: one decode pipeline per clip
    /// under the playhead, opened and driven off this thread. This is what
    /// makes the viewer show real media rather than the bare canvas
    /// (docs/PLAN.md §4).
    previews: PreviewService,
    /// The compositor output as egui knows it, and the canvas it was
    /// registered at, so a resolution change re-registers rather than
    /// stretching a texture that no longer exists.
    preview: Option<(egui::TextureId, Resolution)>,
    /// Whether the playhead has moved since the last composite.
    needs_composite: bool,
    /// The keyboard map every panel's shortcuts come from, and whatever the
    /// user's `keymap.toml` got wrong.
    keymap: LoadedKeymap,
    /// The window listing every binding.
    shortcuts_window: ShortcutsWindow,
    /// The prompt an open shows when an autosave is ahead of the file.
    recovery: RecoveryPrompt,
    /// The restore list in the File menu.
    snapshots: SnapshotMenu,
    /// The local socket this window serves the Command API on, when it serves
    /// one. `None` when the endpoint was switched off or could not even be
    /// named; a refused bind is a [`CommandApi`] that is not serving, which is
    /// a different thing (see [`crate::command_api`]).
    command_api: Option<CommandApi>,
}

impl SubordinateApp {
    /// Build the app from eframe's creation context.
    ///
    /// # Errors
    ///
    /// - `render.no_render_state` when eframe was built without its wgpu
    ///   backend, in which case there is no device to share.
    /// - Whatever starting the engine thread returns.
    pub fn new(cc: &eframe::CreationContext<'_>, options: AppOptions) -> SubResult<Self> {
        let state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or_else(|| render_error(&RenderError::MissingRenderState))?;
        let render = RenderContext::new(
            state.device.clone(),
            state.queue.clone(),
            state.adapter.get_info(),
        );
        log::info!(
            "render device ready on {}: {}",
            render.backend_label(),
            render.describe()
        );
        if render.is_software() {
            log::warn!("no GPU adapter available; falling back to software rendering");
        }
        // The user's keymap.toml overrides the shipped map. A rejected entry,
        // like a double-bound chord, is a configuration problem rather than a
        // reason to refuse to start, so both are logged once here.
        let keymap = LoadedKeymap::load();
        keymap.log_problems();
        keymap.map.log_conflicts();
        let sequence = Sequence::new("Sequence", SequenceSettings::default());
        let compositor = Compositor::for_sequence(render.clone(), &sequence);
        let viewer = ViewerPanel::for_sequence(&sequence);
        let scheduler = PlaybackScheduler::for_sequence(&sequence);
        let timeline = TimelinePanel::new(sequence.settings.frame_rate);
        // The panel arrangement is configuration too: a file that cannot be
        // read costs the user their arrangement, never their session.
        let layout = DockLayout::load();
        layout.log_problems();
        // So is the fullscreen display: a review setup is remembered between
        // sessions, and a file that cannot be read costs the user one click.
        let fullscreen = FullscreenState::load();
        fullscreen.log_problems();
        let meters = Arc::new(MeterBank::new(METERED_TRACKS));
        let audio_control = Rc::new(RefCell::new(None));
        let scrub_control = Rc::new(RefCell::new(None));
        let scrub_settings = Rc::new(Cell::new(ScrubSettings::default()));
        let audio = audio_output(
            sequence.settings.sample_rate,
            Arc::clone(&meters),
            Rc::clone(&audio_control),
            Rc::clone(&scrub_control),
            Rc::clone(&scrub_settings),
        );
        let popout = startup_popout(&options, fullscreen.state.monitor_index());
        let startup_project = options.project.clone();
        let mut app = Self {
            render,
            render_state: state.clone(),
            options,
            frames_painted: 0,
            closing: false,
            started: Instant::now(),
            project_state: ProjectState::None,
            announced_ready: false,
            diagnostics: DiagnosticsPanel::new(),
            audio_settings: AudioSettingsPanel::new(),
            audio,
            meters,
            sequence,
            compositor,
            viewer,
            popout,
            scheduler,
            audio_control,
            scrub_control,
            scrub_settings,
            scrub_until: None,
            session: EditorSession::new(Project::new("Untitled"))?,
            synced: None,
            tabs: SequenceTabs::new(),
            media_bin: MediaBinPanel::new(),
            timeline,
            inspector: InspectorPanel::new(),
            export: ExportHost::new(),
            media: MediaHost::new(),
            jobs: JobService::new(JOB_WORKERS),
            effect_catalog: EffectCatalog::new(),
            layout: layout.layout,
            fullscreen: fullscreen.state,
            previews: PreviewService::new(),
            preview: None,
            needs_composite: true,
            keymap,
            shortcuts_window: ShortcutsWindow::new(),
            recovery: RecoveryPrompt::new(),
            snapshots: SnapshotMenu::new(),
            command_api: None,
        };
        if let Some(path) = startup_project {
            app.open_startup_project(&path);
        }
        app.start_command_api();
        Ok(app)
    }

    /// Opens the project named on the command line, recording how it went.
    ///
    /// A file that will not open is not a reason to refuse to start: the
    /// window comes up empty and the ready line says `failed`, which is what
    /// the unattended smoke run reports.
    fn open_startup_project(&mut self, path: &Path) {
        self.project_state = match self.open_project(path) {
            Ok(()) => {
                log::info!("opened {}", path.display());
                ProjectState::Loaded
            }
            Err(error) => {
                log::error!(
                    "could not open {}: [{}] {}",
                    path.display(),
                    error.code,
                    error.message
                );
                ProjectState::Failed
            }
        };
    }

    /// Starts serving the Command API, when this run is to serve it.
    ///
    /// The bind itself happens on a thread of its own — nothing here may stall
    /// the first frame — so this only names the endpoint and asks for it; the
    /// once-a-frame [`CommandApi::sync`] collects the outcome. An endpoint
    /// this machine cannot even name is logged and the editor runs without an
    /// agent surface, exactly as a refused bind does.
    fn start_command_api(&mut self) {
        if !self.options.serve_command_api {
            log::info!("this editor does not serve the Command API");
            return;
        }
        match self.options.endpoint() {
            Ok(endpoint) => {
                let mut api = CommandApi::new(endpoint);
                api.serve(&self.session);
                self.command_api = Some(api);
            }
            Err(error) => log::warn!(
                "the Command API has no endpoint on this machine: [{}] {}",
                error.code,
                error.message
            ),
        }
    }

    /// The Command API endpoint this window serves, when it serves one.
    #[must_use]
    pub const fn command_api(&self) -> Option<&CommandApi> {
        self.command_api.as_ref()
    }

    /// Opens a project file, offering to recover a newer autosave first.
    ///
    /// The file is read and adopted whatever the autosave history says, so
    /// the editor always ends up showing something; when a snapshot is ahead
    /// of the file, the recovery prompt goes up over it and the user's answer
    /// replaces the project or throws the history away
    /// (`sub_edit::autosave::check_for_recovery`).
    ///
    /// # Errors
    ///
    /// - `ui.project_unreadable` when the file cannot be read.
    /// - Whatever `sub_model::json::from_json` returns for its contents.
    ///
    /// A sidecar directory that cannot be listed is logged rather than
    /// returned: it costs the user their autosave history, never their open.
    pub fn open_project(&mut self, path: &Path) -> Result<(), SubError> {
        // The engine owns the project for its whole life, so opening a file
        // starts a fresh one; the autosave worker and the change-event
        // subscription are rebuilt with it.
        self.session.open(path)?;
        self.adopt_view();
        if let Err(error) = self.snapshots.set_project(path) {
            log::warn!("autosave history: [{}] {}", error.code, error.message);
        }
        match self.recovery.open_for(path) {
            Ok(true) => log::info!("an autosave is newer than {}", path.display()),
            Ok(false) => {}
            Err(error) => log::warn!("autosave check: [{}] {}", error.code, error.message),
        }
        Ok(())
    }

    /// Writes the project back to the file it came from.
    ///
    /// # Errors
    ///
    /// `ui.project_unsaved` when no file is known or the write fails.
    pub fn save_project(&mut self) -> Result<(), SubError> {
        self.session.save()?;
        self.refresh_snapshot_menu();
        Ok(())
    }

    /// Writes the project to `path` and edits that file from now on.
    ///
    /// # Errors
    ///
    /// `ui.project_unsaved` when the write fails.
    pub fn save_project_as(&mut self, path: &Path) -> Result<(), SubError> {
        self.session.save_as(path)?;
        if let Err(error) = self.snapshots.set_project(path) {
            log::warn!("autosave history: [{}] {}", error.code, error.message);
        }
        Ok(())
    }

    /// The engine, the project it owns and the workers hanging off it.
    pub const fn session(&self) -> &EditorSession {
        &self.session
    }

    /// The same session, for a host that drives the editor directly.
    pub const fn session_mut(&mut self) -> &mut EditorSession {
        &mut self.session
    }

    /// The file the open project came from, once one has been opened.
    pub fn project_file(&self) -> Option<&Path> {
        self.session.project_file()
    }

    /// The project as the panels last saw it.
    pub fn project(&self) -> &Project {
        self.session.project()
    }

    /// The sequence the timeline and the viewer are showing.
    pub const fn sequence(&self) -> &Sequence {
        &self.sequence
    }

    /// The autosave recovery prompt.
    pub fn recovery(&mut self) -> &mut RecoveryPrompt {
        &mut self.recovery
    }

    /// The snapshot restore list behind the File menu.
    pub fn snapshots(&mut self) -> &mut SnapshotMenu {
        &mut self.snapshots
    }

    /// Shows `project` in every panel, with the playhead back at the start.
    ///
    /// This is what opening a file and restoring a snapshot both do: a
    /// snapshot is a whole project rather than an edit to one, so it replaces
    /// the engine instead of going through the history as a command. The undo
    /// stack of the project being closed goes with it, which is what an
    /// editor does when a file is closed.
    ///
    /// # Errors
    ///
    /// Whatever starting the engine thread returns.
    pub fn adopt_project(&mut self, project: Project) -> Result<(), SubError> {
        let file = self.session.project_file().map(Path::to_path_buf);
        self.session.adopt(project, file)?;
        self.adopt_view();
        Ok(())
    }

    /// Rebuilds every view that follows the project after the engine has been
    /// replaced.
    ///
    /// The panels themselves are rebuilt rather than synced, because a new
    /// project shares nothing with the old one: a remembered zoom or selection
    /// would name clips that no longer exist.
    fn adopt_view(&mut self) {
        self.tabs = SequenceTabs::new();
        self.synced = None;
        let opening = self
            .session
            .project()
            .sequences
            .first()
            .cloned()
            .unwrap_or_else(|| Sequence::new("Sequence", SequenceSettings::default()));
        self.viewer = ViewerPanel::for_sequence(&opening);
        self.timeline = TimelinePanel::new(opening.settings.frame_rate);
        self.sequence = opening;
        self.sync_project();
    }

    /// Reconciles the tab strip and the cached sequence with the engine.
    ///
    /// Called once a frame and after every command: reading the project is a
    /// snapshot read rather than a lock, so doing it every frame costs a
    /// pointer clone, and the revision comparison keeps everything downstream
    /// of it off the frame that changed nothing.
    fn sync_project(&mut self) {
        let active = self.tabs.sync(&self.session.project().sequences);
        let key = (self.session.revision(), active);
        if self.synced == Some(key) {
            return;
        }
        let switched = self.synced.is_none_or(|(_, was)| was != active);
        self.synced = Some(key);
        let sequence = active
            .and_then(|id| {
                self.session
                    .project()
                    .sequences
                    .iter()
                    .find(|sequence| sequence.id == id)
            })
            .cloned()
            .unwrap_or_else(|| Sequence::new("Sequence", SequenceSettings::default()));
        if switched || self.compositor.resolution() != sequence.settings.resolution {
            // The compositor's output texture belongs to the sequence it was
            // built for, so the egui registration of the old one has to go
            // with it.
            self.free_preview();
            self.compositor = Compositor::for_sequence(self.render.clone(), &sequence);
        }
        if switched {
            // The pipelines open belong to the clips of the sequence being
            // left; the new one's clips open their own.
            self.previews.clear();
            // Where this tab was last being looked at, or its origin the first
            // time it is shown.
            let state = active
                .and_then(|id| self.tabs.remembered(id).copied())
                .unwrap_or_else(|| SequenceViewState::initial(&sequence));
            state.restore(&sequence, &mut self.timeline, &mut self.viewer.state);
            self.scheduler = PlaybackScheduler::for_sequence(&sequence);
        } else {
            // An edit can lengthen or shorten the sequence under the playhead.
            self.viewer.state.set_duration(sequence_duration(&sequence));
        }
        self.sequence = sequence;
        self.needs_composite = true;
    }

    /// Drops the egui registration of the compositor's output, if there is
    /// one, so the next composite registers the current texture.
    fn free_preview(&mut self) {
        if let Some((stale, _)) = self.preview.take() {
            self.render_state.renderer.write().free_texture(&stale);
        }
    }

    /// Re-reads the autosave history behind the File menu.
    fn refresh_snapshot_menu(&mut self) {
        if let Err(error) = self.snapshots.refresh() {
            log::warn!("autosave history: [{}] {}", error.code, error.message);
        }
    }

    /// Applies what the recovery prompt or the restore list handed back.
    ///
    /// A failure is logged and shown by the widget that raised it; it never
    /// disturbs the project already open.
    fn apply_recovery(&mut self, outcome: Option<RecoveryOutcome>) {
        match outcome {
            Some(RecoveryOutcome::Recovered(project)) => {
                if let Err(error) = self.adopt_project(*project) {
                    log::warn!("recovery: [{}] {}", error.code, error.message);
                }
                self.refresh_snapshot_menu();
            }
            Some(RecoveryOutcome::Discarded) => self.refresh_snapshot_menu(),
            Some(RecoveryOutcome::Failed(error)) => {
                log::warn!("autosave: [{}] {}", error.code, error.message);
            }
            None => {}
        }
    }

    /// The wgpu device shared with the compositor.
    pub fn render_context(&self) -> &RenderContext {
        &self.render
    }

    /// The hardware diagnostics panel.
    pub fn diagnostics(&mut self) -> &mut DiagnosticsPanel {
        &mut self.diagnostics
    }

    /// The audio settings panel.
    pub fn audio_settings(&mut self) -> &mut AudioSettingsPanel {
        &mut self.audio_settings
    }

    /// The audio output stage.
    pub fn audio(&mut self) -> &mut AudioOutput<CpalBackend> {
        &mut self.audio
    }

    /// Feeds the audio settings panel and applies what it asks for.
    ///
    /// A failed switch is reported in the panel rather than propagated: the
    /// output has already put the previous device back, so the editor carries
    /// on playing.
    fn apply_audio_settings(&mut self, ctx: &egui::Context) {
        if self.audio_settings.needs_devices() {
            let devices = self.audio.devices();
            self.audio_settings.set_devices(devices);
        }
        self.audio_settings
            .set_diagnostics(self.audio.diagnostics());
        self.audio_settings.set_scrub(self.scrub_settings.get());
        match self.audio_settings.show(ctx) {
            AudioSettingsAction::None => {}
            AudioSettingsAction::Rescan => self.audio_settings.refresh(),
            AudioSettingsAction::SelectDevice(device_id) => {
                let result = self.audio.select_device(device_id.as_deref());
                self.audio_settings.set_error(result.err());
                self.audio_settings
                    .set_selected(self.audio.selected_device());
            }
            AudioSettingsAction::SetScrub(settings) => self.apply_scrub_settings(settings),
        }
    }

    /// Applies scrub settings to the player, if a stream is open, and keeps
    /// them for the next stream that opens.
    ///
    /// Settings the player refuses — a grain outside its range — are reported
    /// in the panel and not kept, so the widgets snap back to what is really
    /// in force rather than lying about it.
    fn apply_scrub_settings(&mut self, settings: ScrubSettings) {
        if let Err(error) = settings.validate() {
            self.audio_settings.set_error(Some(error));
            return;
        }
        if let Some(control) = self.scrub_control.borrow().as_ref()
            && let Err(error) = control.apply(settings)
        {
            self.audio_settings.set_error(Some(error));
            return;
        }
        self.scrub_settings.set(settings);
    }

    /// Plays one grain of the mix at `position`, the way an NLE sounds while
    /// the playhead is dragged (docs/PLAN.md §5.4).
    ///
    /// The output stage is opened if it is closed, because a drag is the one
    /// thing that makes a sound while nothing is playing, and held open until
    /// the grain has finished. With scrubbing turned off in the settings this
    /// does nothing at all: no stream is opened and no grain is asked for.
    fn scrub_audio(&mut self, position: RationalTime) {
        let settings = self.scrub_settings.get();
        if !settings.enabled {
            return;
        }
        if !self.audio.is_open()
            && let Err(error) = self.audio.start()
        {
            log::warn!(
                "no audio output; scrubbing is silent: [{}] {}",
                error.code,
                error.message
            );
            return;
        }
        if let Some(control) = self.scrub_control.borrow().as_ref() {
            match control.grain_at(position) {
                Ok(()) => self.scrub_until = Some(Instant::now() + grain_duration(settings)),
                Err(error) => log::warn!(
                    "could not scrub the audio: [{}] {}",
                    error.code,
                    error.message
                ),
            }
        }
    }

    /// Whether a scrub grain asked for earlier may still be sounding.
    fn scrub_is_sounding(&self) -> bool {
        self.scrub_until.is_some_and(|until| Instant::now() < until)
    }

    /// The viewer panel, which owns the playhead.
    pub fn viewer(&mut self) -> &mut ViewerPanel {
        &mut self.viewer
    }

    /// The compositor whose output the viewer paints.
    ///
    /// Public for the same reason the viewer is: a host — or a test driving
    /// the window without a pointer — has to be able to look at the picture
    /// the window is showing, and [`Compositor::read_rgba`] is the only way to
    /// read a texture the GPU owns.
    pub const fn compositor(&self) -> &Compositor {
        &self.compositor
    }

    /// The decode pipelines behind the viewer's picture.
    ///
    /// Public so a caller with no pointer can tell whether the preview has
    /// caught up with the playhead — [`crate::preview::PreviewStats::late`] against a settled
    /// picture — rather than guessing with a sleep.
    pub const fn previews(&self) -> &PreviewService {
        &self.previews
    }

    /// The media bin panel, which owns what looking at the bin means and what
    /// the host's import and relink jobs are doing.
    ///
    /// Public for the same reason the viewer is: a host — or a test driving
    /// the window without a pointer — reads the pending rows and the failures
    /// the panel is showing rather than a log.
    pub fn media_bin(&mut self) -> &mut MediaBinPanel {
        // The panel is normally told what the jobs are doing once a frame,
        // just before it paints. A caller reaching for it between frames
        // wants that answer as it stands now rather than as it stood when the
        // last frame started, so the status is refreshed on the way out.
        self.media_bin.set_status(self.media.status());
        &mut self.media_bin
    }

    /// The relink dialog, open for one item or for every offline one.
    ///
    /// Public so a caller can tell the dialog which file replaces an offline
    /// item without a native file dialog, which is what a test does and what
    /// an agent surface would do.
    pub fn relink_dialog(&mut self) -> &mut RelinkDialog {
        &mut self.media.relink
    }

    /// Starts the relink search of `folder` on the window's job pool.
    ///
    /// The same thing the dialog's "Search folder..." button does once the
    /// native folder picker has answered, without the picker: hashing a card
    /// of footage is a job, and this is how a caller with no pointer asks for
    /// it.
    pub fn search_for_relink(&mut self, folder: impl Into<PathBuf>) {
        self.media.relink.search_folder(&self.jobs, folder);
    }

    /// The timeline panel, which owns the clip selection.
    ///
    /// The selection is what the inspector edits, so this is also how another
    /// surface — a test, or a future selection command — says which clips the
    /// parameter fields are pointing at.
    pub fn timeline(&mut self) -> &mut TimelinePanel {
        &mut self.timeline
    }

    /// The viewer's pop-out window.
    pub fn popout(&mut self) -> &mut PopoutViewer {
        &mut self.popout
    }

    /// Which display the pop-out goes fullscreen on.
    pub const fn fullscreen(&mut self) -> &mut FullscreenState {
        &mut self.fullscreen
    }

    /// Applies what the monitor picker asked for.
    ///
    /// The picker has already recorded the chosen display in
    /// [`SubordinateApp::fullscreen`]; this is what the pop-out window is told
    /// about it, and what writes the choice out so the next session starts on
    /// the same display.
    fn apply_fullscreen(&mut self, action: Option<FullscreenAction>) {
        let Some(action) = action else {
            return;
        };
        match action {
            FullscreenAction::Choose(_) => {
                self.popout.set_monitor(self.fullscreen.monitor_index());
            }
            FullscreenAction::Enter => {
                self.popout.set_monitor(self.fullscreen.monitor_index());
                self.popout.set_fullscreen(true);
            }
            FullscreenAction::Leave => self.popout.set_fullscreen(false),
        }
        match self.fullscreen.persist() {
            Ok(true) => log::debug!("fullscreen display saved"),
            Ok(false) => {}
            Err(error) => log::warn!("fullscreen: [{}] {}", error.code, error.message),
        }
    }

    /// Runs the pop-out window for this frame and applies what it saw.
    ///
    /// The pop-out shares the compositor's texture rather than compositing
    /// again, so all that crosses back is the picture's handle one way and
    /// the playback keys the other. Returns whether the playhead moved.
    fn run_popout(&mut self, ctx: &egui::Context, preview: ViewerFrame) -> bool {
        self.popout.publish(preview);
        let open = self.popout.show(ctx, &self.keymap.map);
        self.viewer.popped_out = open;
        let mut moved = false;
        for action in self.popout.take_actions() {
            if let Some(viewer_action) = ViewerAction::for_action(action) {
                moved |= self.viewer.state.apply(viewer_action);
            } else if let Some(transport) = TransportAction::for_action(action) {
                transport.apply(&mut self.scheduler);
            }
        }
        moved
    }

    /// The keyboard map in force, after any `keymap.toml` overrides.
    pub fn shortcuts(&self) -> &ShortcutMap {
        &self.keymap.map
    }

    /// Everything the user's `keymap.toml` got wrong, as loaded at startup.
    pub fn keymap_problems(&self) -> &[SubError] {
        &self.keymap.problems
    }

    /// Runs the keyboard map for this frame and applies what it fired.
    ///
    /// Returns true when the playhead moved, so the caller composites again.
    /// An action this build has no home for is logged and dropped rather than
    /// swallowed silently.
    fn apply_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        let mut moved = false;
        for action in self.keymap.map.poll(ctx) {
            if let Some(viewer_action) = ViewerAction::for_action(action) {
                moved |= self.viewer.state.apply(viewer_action);
            } else if let Some(transport) = TransportAction::for_action(action) {
                transport.apply(&mut self.scheduler);
                log::debug!("transport now {}", self.scheduler.speed().label());
            } else if action == Action::ShowShortcutHelp {
                self.shortcuts_window.toggle();
            } else if action == Action::ToggleSnapping {
                let on = self.timeline.toggle_snapping();
                log::debug!("timeline snapping is now {}", if on { "on" } else { "off" });
            } else if action == Action::SelectTool {
                self.timeline.set_tool(Tool::Select);
            } else if action == Action::RazorTool {
                self.timeline.set_tool(Tool::Razor);
            } else if action == Action::SplitAtPlayhead {
                // The playhead a cut lands on is the viewer's, which the
                // timeline is handed before every frame; the panel plans the
                // cut on the next one, when it has the sequence to plan
                // against.
                self.timeline.set_playhead(self.viewer.state.playhead());
                self.timeline.request_split_at_playhead();
            } else if action == Action::AddMarker {
                // The playhead the marker lands on is the viewer's, which the
                // timeline is handed before every frame; the panel raises the
                // add as a command on the next one.
                self.timeline.set_playhead(self.viewer.state.playhead());
                let marker = self.timeline.add_marker_at_playhead();
                log::debug!("marker {marker} dropped at the playhead");
            } else if action == Action::Undo {
                self.perform_history(HistoryAction::Undo { steps: 1 });
            } else if action == Action::Redo {
                self.perform_history(HistoryAction::Redo { steps: 1 });
            } else if let Some(mode) = edit_mode_for(action) {
                self.edit_from_bin(mode);
            } else {
                // In and out points and nudging have no panel in this build;
                // they are logged rather than swallowed so a press that does
                // nothing says why.
                log::debug!("shortcut {} has no home in this build", action.id());
            }
        }
        moved
    }

    /// Edits the bin's selected item onto the target track at the playhead.
    ///
    /// Comma inserts and period overwrites, both at the playhead and both on
    /// the lane the editor last pointed at
    /// ([`TimelinePanel::target_track`](crate::timeline_panel::TimelinePanel::target_track)).
    /// The plan is one command, applied through the engine handle. A refusal —
    /// a locked track, an item with no picture on a video track — is reported
    /// rather than applied.
    fn edit_from_bin(&mut self, mode: EditMode) {
        let project = self.session.project_arc();
        let BinSelection::Media(media) = self.media_bin.selection(&project) else {
            log::debug!("{}: nothing is selected in the bin", mode.id());
            return;
        };
        self.timeline.set_playhead(self.viewer.state.playhead());
        match self
            .timeline
            .plan_edit_at_playhead(&project, &self.sequence, media, mode)
        {
            Ok(plan) => {
                let _ = self.session.apply_boxed(plan.into_command());
                self.sync_project();
            }
            Err(refusal) => log::debug!("{}: {}", mode.id(), refusal.message()),
        }
    }

    /// Undoes or redoes the steps `action` asks for, through the engine.
    ///
    /// The engine is the only history there is, so the Edit menu, the keyboard
    /// and the Command API all move the same stack.
    fn perform_history(&mut self, action: HistoryAction) {
        for _ in 0..action.steps() {
            let stepped = match action {
                HistoryAction::Undo { .. } => self.session.undo(),
                HistoryAction::Redo { .. } => self.session.redo(),
            };
            if !matches!(stepped, Ok(true)) {
                break;
            }
        }
        self.sync_project();
    }

    /// Runs the transport for this frame and returns whether the playhead
    /// moved.
    ///
    /// Audio is the master (docs/PLAN.md §5.4): while the output stream is
    /// playing, the frame shown is the one covering the position the callback
    /// has actually rendered, less what is still sitting in the device buffer.
    /// With no stream — nothing playing at 1x, or no device to open — the
    /// scheduler follows its monotonic fallback master instead, advanced by
    /// the wall time egui reports for the frame. Either way any other move —
    /// a scrub, a frame step — is fed the other way, so playback resumes from
    /// wherever the user left the playhead, and presentations the master ran
    /// past are dropped and counted rather than slowing playback down.
    fn run_transport(&mut self, ctx: &egui::Context, elapsed: Duration) -> bool {
        self.scheduler.set_duration(self.viewer.state.duration());
        if !self.scheduler.is_playing() {
            // A scrub grain is the one thing that sounds while nothing is
            // playing, so the stream stays open until it has finished.
            if self.scrub_is_sounding() {
                ctx.request_repaint();
            } else {
                self.scrub_until = None;
                self.stop_audio();
            }
            self.scheduler.seek(self.viewer.state.playhead());
            return false;
        }
        if self.scheduler.position() != self.viewer.state.playhead() {
            // The user scrubbed or stepped while playing; carry on from there.
            self.scheduler.seek(self.viewer.state.playhead());
            self.seek_audio(self.viewer.state.playhead());
        }
        self.follow_audio();
        let master = self
            .audio
            .clock()
            .and_then(|clock| clock.position())
            .map(|position| position.rescaled_to(self.scheduler.rate()));
        let tick = match master {
            Some(position) => self.scheduler.follow(position),
            None => self.scheduler.advance(elapsed),
        };
        let moved = tick.is_some_and(|tick| {
            if tick.wrapped || tick.stopped {
                // The master has to be moved with the playhead: a loop that
                // wrapped the picture and left the sound running would not be
                // in sync any more.
                self.seek_audio(tick.position);
            }
            self.viewer.state.seek_to(tick.position)
        });
        // Playback only looks like playback if the next frame is asked for.
        ctx.request_repaint();
        moved
    }

    /// Opens the output stream at the playhead once playback is running at
    /// 1x, and closes it at any other speed.
    ///
    /// Only 1x is played out: shuttling and reverse have no audio until
    /// scrubbing lands (TASK-54), so at those speeds the stream is closed and
    /// the fallback master drives the picture. A device that will not open is
    /// logged once and playback carries on silently rather than stopping.
    fn follow_audio(&mut self) {
        if self.scheduler.speed() != ShuttleSpeed::Forward1x {
            self.stop_audio();
            return;
        }
        if self.audio.is_open() {
            return;
        }
        if let Err(error) = self.audio.start() {
            log::warn!(
                "no audio output; playback follows the monotonic clock: [{}] {}",
                error.code,
                error.message
            );
            return;
        }
        self.seek_audio(self.viewer.state.playhead());
    }

    /// Closes the output stream, if one is open, and drops the master
    /// baseline so reopening it is not read as dropped frames.
    fn stop_audio(&mut self) {
        if !self.audio.is_open() {
            return;
        }
        self.audio.stop();
        self.scheduler.resync_master();
    }

    /// Moves the audio transport to `position` and forgets the clock reading
    /// taken at the old one.
    fn seek_audio(&mut self, position: RationalTime) {
        if let Some(control) = self.audio_control.borrow_mut().as_mut()
            && let Err(error) = control.seek(position)
        {
            log::warn!(
                "could not move the audio transport: [{}] {}",
                error.code,
                error.message
            );
        }
        if let Some(clock) = self.audio.clock() {
            clock.reset();
        }
        self.scheduler.resync_master();
    }

    /// Composites the sequence at the playhead and returns the picture the
    /// viewer should sample.
    ///
    /// The layers come from [`crate::preview::PreviewService`]: one decode
    /// pipeline per clip under the playhead, each on a worker of its own, each
    /// seeked by the playhead while scrubbing and fed by its decode-ahead ring
    /// while the transport runs. Nothing here waits for a decoder — a clip
    /// with no picture ready contributes no layer, exactly as a gap does — so
    /// this stays a compositor pass and an upload however slow the media is.
    ///
    /// A composite is redone when the playhead has moved *or* a clip has
    /// delivered a new picture, which is what puts a frame on screen when a
    /// worker finishes between paints. The output texture is registered with
    /// egui once and re-registered only when the canvas size changes, because
    /// [`Compositor::render`] otherwise keeps drawing into the same texture.
    fn composite(&mut self, ctx: &egui::Context) -> ViewerFrame {
        let playhead = self.viewer.state.playhead();
        let project = self.session.project_arc();
        let project_dir = self.project_dir();
        self.previews.set_playing(self.scheduler.is_playing());
        self.previews.set_media_use(self.viewer.media_use());
        self.previews.set_cache_dir(
            self.session
                .project_file()
                .and_then(|file| sub_edit::autosave::sidecar_dir(file).ok()),
        );
        let pictures = self.previews.pictures(
            &self.render,
            &self.jobs,
            &project,
            project_dir.as_deref(),
            &self.sequence,
            playhead,
        );
        if pictures.busy() {
            // A worker landing a picture between paints has no way to wake
            // egui, so a preview that is still catching up asks for the next
            // frame itself. This is the only thing that waits on decode, and
            // it waits by painting again rather than by blocking.
            ctx.request_repaint_after(PREVIEW_POLL_INTERVAL);
        }
        if self.needs_composite || pictures.changed() {
            let mut source =
                |layer: &ResolvedClip<'_>| -> Option<SourceFrame> { pictures.get(layer.clip_id()) };
            self.compositor
                .render(&self.sequence, playhead, &mut source);
            self.needs_composite = false;
        }
        let resolution = self.compositor.resolution();
        if self.preview.is_none_or(|(_, known)| known != resolution) {
            let mut renderer = self.render_state.renderer.write();
            if let Some((stale, _)) = self.preview.take() {
                renderer.free_texture(&stale);
            }
            let view = self
                .compositor
                .output()
                .create_view(&wgpu::TextureViewDescriptor::default());
            let texture = renderer.register_native_texture(
                self.render.device(),
                &view,
                wgpu::FilterMode::Linear,
            );
            self.preview = Some((texture, resolution));
        }
        let (texture, resolution) = self
            .preview
            .unwrap_or((egui::TextureId::default(), resolution));
        ViewerFrame::new(texture, resolution.width(), resolution.height())
    }

    /// Draws the docked panels and returns whether the playhead moved.
    ///
    /// The dock owns the arrangement; each panel's body is drawn here, so a
    /// panel dragged into another split or grouped into a tab keeps working
    /// exactly as it did.
    fn dock_ui(&mut self, ui: &mut egui::Ui, preview: ViewerFrame) -> bool {
        // The project is read once for the whole frame as an immutable
        // snapshot, so every panel sees the same project and none of them
        // borrows the session the dispatch below needs.
        let project = self.session.project_arc();
        let revision = self.session.revision();
        let mut frame = FrameEdits::default();
        // Destructured so each panel body borrows the fields it draws with
        // while the dock borrows the layout.
        let Self {
            layout,
            viewer,
            media_bin,
            timeline,
            inspector,
            export,
            media,
            effect_catalog,
            sequence,
            ..
        } = self;
        // The engine's revision counter is what tells the timeline its cached
        // layout is stale, so an edit made anywhere — a panel, the Command
        // API, a plugin — invalidates it.
        // The bin draws what the host's jobs are doing — the files still
        // being probed, and what the last import or relink got wrong — so it
        // is told before it paints.
        media_bin.set_status(media.status());
        timeline.sync(sequence, revision);
        // The viewer owns the playhead; the timeline draws it and can ask for
        // a new one, so it is handed the current value before it paints.
        timeline.set_playhead(viewer.state.playhead());
        let mut moved = false;
        layout.ui(ui, |ui, panel| match panel {
            Panel::Viewer => moved |= viewer.ui(ui, Some(preview)),
            Panel::MediaBin => frame.bin.extend(media_bin.ui(ui, &project)),
            Panel::Timeline => {
                let response = timeline.ui(ui, &project, sequence);
                if let Some(time) = response.seek {
                    // Clicking or scrubbing the ruler moves the playhead,
                    // which is view state rather than part of the edit, so it
                    // is a seek and not a Command.
                    moved |= viewer.state.seek_to(time);
                }
                frame.timeline = Some(response);
            }
            Panel::Inspector => {
                let response = inspector.ui(ui, sequence, timeline.selection(), effect_catalog);
                if !response.is_empty() {
                    frame.inspector = Some(response);
                }
            }
            Panel::Export => {
                // The panel opens on whatever sequence is on screen until the
                // user picks another one for themselves.
                if export.panel.sequence().is_none() {
                    export.panel.select_sequence(&project, sequence.id);
                }
                frame.export = export.panel.ui(ui, &project);
            }
        });
        // Everything the panels asked for is applied here, once the dock has
        // given the borrows back: each gesture becomes one entry in the undo
        // stack, on the engine every other client edits through.
        self.apply_frame(frame);
        moved
    }

    /// The export panel: the preset, range and file an export is started
    /// with, and what the running one is doing.
    ///
    /// Public so a host — or a test that exports without a pointer — can set
    /// up an export and read its progress, the way the panel's own widgets do.
    pub const fn export_panel(&mut self) -> &mut ExportPanel {
        &mut self.export.panel
    }

    /// What the last export asked for is doing, or did.
    pub const fn export_status(&self) -> &crate::export_panel::ExportStatus {
        self.export.panel.status()
    }

    /// Carries out what the export panel asked for.
    ///
    /// Nothing here edits the project, so none of it is a command: an export
    /// reads the project and writes a file.
    ///
    /// Public because this, not `start_export`, is what a click on Export
    /// reaches: a caller driving the window without a pointer takes the same
    /// path a user does.
    pub fn apply_export(&mut self, action: ExportAction) {
        match action {
            ExportAction::Start(request) => self.start_export(&request),
            ExportAction::Cancel => self.export.runner.cancel(),
            ExportAction::ChooseOutput => {
                if let Some(path) = pick_export_destination(self.export.panel.output()) {
                    self.export.panel.set_output(path);
                }
            }
            ExportAction::Reveal(path) => {
                if let Err(error) = crate::export_panel::reveal(&path) {
                    log::warn!("[{}] {}", error.code, error.message);
                    self.export.panel.add_problem(error);
                }
            }
        }
    }

    /// Hands `request` to the export job, and its refusal to the panel.
    ///
    /// The job reports itself from here on: every progress snapshot, the ETA
    /// and the terminal event reach the panel through
    /// [`SubordinateApp::poll_export`], and the panel's Cancel button reaches
    /// the job's cancel token.
    fn start_export(&mut self, request: &ExportRequest) {
        let Some(project_dir) = self.project_dir() else {
            let error = SubError::new(crate::codes::EXPORT_NOT_READY, NO_RENDERER_REASON)
                .with_detail("field", "project");
            log::warn!("[{}] {}", error.code, error.message);
            self.export.panel.add_problem(error);
            return;
        };
        let project = self.session.project_arc();
        let render = self.render.clone();
        let jobs = &self.jobs;
        let export = &mut self.export;
        let started = export.runner.start(
            jobs,
            &export.presets,
            &project,
            request,
            &mut crate::export_runner::sequence_sources(&render, Arc::clone(&project), project_dir),
        );
        if let Err(error) = started {
            log::warn!("[{}] {}", error.code, error.message);
            export.panel.add_problem(error);
        }
    }

    /// The folder a clip's relative media path resolves against.
    ///
    /// That is the folder the project file itself lives in, so a project that
    /// has never been saved has none and cannot be exported
    /// ([`NO_RENDERER_REASON`]).
    fn project_dir(&self) -> Option<PathBuf> {
        let file = self.session.project_file()?;
        let folder = file.parent().unwrap_or(Path::new("."));
        // Absolute, because the decoders open a URI and GStreamer takes no
        // relative path. Plain, because on Windows `canonicalize` answers with
        // a verbatim `\\?\C:\...` path, which GLib will not turn into a
        // `file://` URI and which matches nothing a file dialog hands back.
        std::fs::canonicalize(folder)
            .ok()
            .map(sub_model::plain_path)
    }

    /// Drains the running export's events into the panel. Once a frame.
    ///
    /// The Export button's availability is refreshed here too: a project only
    /// becomes exportable once it has a file of its own to resolve its media
    /// against, which saving it gives it.
    fn poll_export(&mut self) -> bool {
        let unavailable = self
            .session
            .project_file()
            .is_none()
            .then_some(NO_RENDERER_REASON);
        let export = &mut self.export;
        export.panel.set_unavailable(unavailable);
        export.runner.poll(&mut export.panel) > 0
    }

    /// Applies everything one frame's panels asked for.
    fn apply_frame(&mut self, frame: FrameEdits) {
        for action in frame.bin {
            self.apply_bin_action(action);
        }
        if let Some(response) = frame.timeline {
            self.apply_timeline(response);
        }
        if let Some(response) = frame.inspector {
            self.apply_inspector(response);
        }
        if let Some(action) = frame.export {
            self.apply_export(action);
        }
        self.sync_project();
    }

    /// Applies one media-bin action.
    ///
    /// Four of the six are one command each and are applied here and now.
    /// The other two are jobs: importing has to hash and probe the files
    /// before there is an item to add, and relinking has to find the
    /// replacement before there is a path to point at. Both are started here
    /// and finish in [`SubordinateApp::poll_media`], where what they produced
    /// becomes one history group.
    ///
    /// Public because this is what a click in the bin reaches: a caller
    /// driving the window without a pointer — an OS drop, or a test — takes
    /// exactly the path a user does.
    pub fn apply_bin_action(&mut self, action: MediaBinAction) {
        match action {
            MediaBinAction::Import { paths, bin } => self.start_import(&paths, bin),
            MediaBinAction::Relink(media) => {
                let project = self.session.project_arc();
                self.media.relink.open_for(&project, media);
            }
            MediaBinAction::RelinkAll => {
                let project = self.session.project_arc();
                self.media.relink.open_for_offline(&project);
            }
            // Named rather than caught by a wildcard, so a variant added to
            // the bin later fails to compile here instead of being silently
            // dropped — which is the bug this whole task is about.
            edit @ (MediaBinAction::CreateBin { .. }
            | MediaBinAction::RenameBin { .. }
            | MediaBinAction::MoveBin { .. }
            | MediaBinAction::MoveMedia { .. }) => {
                if let Some(command) = edit.into_command()
                    && let Err(error) = self.session.apply_boxed(command)
                {
                    self.media.problem(error);
                }
            }
        }
    }

    /// Queues `paths` as one import gesture, filed in `bin`.
    ///
    /// Nothing is read on this thread: each file becomes a job that hashes it,
    /// probes it and builds the item, and the bin shows it as pending until
    /// that lands. The whole gesture is one batch, so it becomes one entry in
    /// the undo stack however many files it carried.
    fn start_import(&mut self, paths: &[PathBuf], bin: BinId) {
        if paths.is_empty() {
            return;
        }
        if !self.ready_to_import() {
            self.media.problem(
                SubError::new(crate::codes::IMPORT_NOT_READY, NO_IMPORT_REASON)
                    .with_detail("field", "project"),
            );
            return;
        }
        // Two disjoint fields: the pool the jobs run on, and the queue that
        // holds them.
        let jobs = &self.jobs;
        if let Some(queue) = self.media.queue.as_mut() {
            queue.submit(jobs, paths, Some(bin));
        }
    }

    /// Makes sure the import queue matches the project as it stands, and
    /// says whether this project can be imported into at all.
    ///
    /// False for a project that has never been saved: an imported path is
    /// stored relative to the project file, so there is nothing to be relative
    /// to and nowhere to put the thumbnail and waveform sidecars.
    fn ready_to_import(&mut self) -> bool {
        let Some(file) = self.session.project_file().map(Path::to_path_buf) else {
            return false;
        };
        let (Some(dir), Ok(cache)) = (self.project_dir(), sub_edit::autosave::sidecar_dir(&file))
        else {
            return false;
        };
        if self.media.dir.as_ref() != Some(&dir) {
            // A queue built for the old folder would express new imports
            // relative to it, so it is replaced rather than reused. Anything
            // it still had in flight is asked to stop.
            if let Some(stale) = self.media.queue.take() {
                stale.cancel_all();
            }
            self.media.queue = Some(ImportQueue::new(&dir, cache));
            self.media.dir = Some(dir);
        }
        self.media.queue.is_some()
    }

    /// Drains the finished imports and the relink search into the project.
    /// Once a frame.
    ///
    /// Returns whether anything happened, so the window knows to repaint while
    /// a job is still running.
    fn poll_media(&mut self) -> bool {
        let mut busy = false;
        // The borrow on the queue ends with this block, because applying what
        // it produced goes through the session and the bin.
        let finished = {
            let jobs = &self.jobs;
            match self.media.queue.as_mut() {
                Some(queue) => {
                    let finished = queue.poll(jobs);
                    busy |= !queue.is_idle();
                    finished
                }
                None => Vec::new(),
            }
        };
        for batch in finished {
            self.apply_import(batch);
        }
        busy |= self.media.relink.poll() || self.media.relink.is_searching();
        busy
    }

    /// Applies one finished import gesture as a single undo step.
    ///
    /// Every file that was hashed and probed becomes an
    /// [`ImportMedia`](sub_edit::commands::ImportMedia) command, and the whole
    /// gesture goes in as one group. Files that could not be read add nothing
    /// and are shown in the bin with the code and message the probe reported,
    /// so an unreadable file is a visible refusal rather than a log line.
    fn apply_import(&mut self, batch: FinishedImport) {
        let mut commands: Vec<sub_edit::BoxedCommand> = Vec::new();
        let mut first = None;
        for outcome in batch.outcomes {
            match outcome {
                ImportOutcome::Ready { item, bin } => {
                    first.get_or_insert(item.id);
                    let command = sub_edit::commands::ImportMedia::new(*item);
                    let command = match bin.or(batch.bin) {
                        Some(bin) => command.into_bin(bin),
                        None => command,
                    };
                    commands.push(Box::new(command));
                }
                ImportOutcome::Failed { path, error } => {
                    self.media
                        .problem(error.with_detail("path", path.display().to_string().as_str()));
                }
            }
        }
        if commands.is_empty() {
            return;
        }
        if let Err(error) = self.session.apply_group(IMPORT_GROUP_LABEL, commands) {
            self.media.problem(error);
            return;
        }
        // The bin opens on what was just imported, which is what a user
        // expects to be looking at after choosing files.
        if let Some(media) = first {
            self.media_bin.select_media(media);
        }
    }

    /// Draws the relink dialog and applies what it decided.
    ///
    /// The search itself is a job; this only puts the window up and turns the
    /// plan it hands back into one history group, so relinking twenty items is
    /// one press of undo.
    fn relink_ui(&mut self, ctx: &egui::Context) {
        if !self.media.relink.is_open() {
            return;
        }
        let project = self.session.project_arc();
        let Some(dir) = self.project_dir() else {
            self.media.relink.close();
            self.media.problem(SubError::new(
                crate::codes::IMPORT_NOT_READY,
                NO_IMPORT_REASON,
            ));
            return;
        };
        // The dialog puts its own window up; this only hands it the pool its
        // search runs on and applies what it decided.
        let Some(plan) = self.media.relink.ui(ctx, &self.jobs, &project, &dir) else {
            return;
        };
        for (_, error) in plan.rejected {
            self.media.problem(error);
        }
        let commands: Vec<sub_edit::BoxedCommand> = plan
            .relinks
            .into_iter()
            .map(|command| Box::new(command) as sub_edit::BoxedCommand)
            .collect();
        if commands.is_empty() {
            return;
        }
        if let Err(error) = self
            .session
            .apply_group(sub_edit::relink::RELINK_GROUP_LABEL, commands)
        {
            self.media.problem(error);
        }
    }

    /// Applies everything a timeline frame asked for.
    ///
    /// Each gesture is planned by the panel and applied here, so one gesture
    /// is exactly one entry in the undo stack. A refusal is not an error: it
    /// is the panel saying the drag under the pointer cannot become an edit,
    /// and it is logged with its reason rather than applied.
    fn apply_timeline(&mut self, response: TimelineResponse) {
        let Some(sequence) = self.tabs.active() else {
            return;
        };
        self.apply_track_actions(sequence, &response);
        if let Some(refusal) = response.refused {
            log::debug!("clip drag refused: {}", refusal.id());
        }
        if let Some(group) = response.clip_move {
            let _ = self
                .session
                .apply_group(group.label.clone(), group.commands());
        }
        if let Some(refusal) = response.trim_refused {
            log::debug!("clip trim refused: {}", refusal.id());
        }
        if let Some(group) = response.clip_trim {
            let _ = self
                .session
                .apply_group(group.label.clone(), group.commands());
        }
        if let Some(refusal) = response.transition_refused {
            log::debug!("crossfade drag refused: {}", refusal.id());
        }
        if let Some(drag) = response.transition {
            let _ = self.session.apply(drag.command);
        }
        if let Some(refusal) = response.fade_refused {
            log::debug!("clip fade refused: {}", refusal.id());
        }
        if let Some(edit) = response.clip_fade {
            let _ = self.session.apply(edit.command);
        }
        if let Some(refusal) = response.split_refused {
            log::debug!("clip split refused: {}", refusal.id());
        }
        if let Some(group) = response.clip_split {
            let _ = self
                .session
                .apply_group(group.label.clone(), group.commands());
        }
        if let Some(refusal) = response.drop_refused {
            log::debug!("bin drop refused: {}", refusal.message());
        }
        if let Some(plan) = response.source_edit {
            let _ = self.session.apply_boxed(plan.into_command());
        }
        for action in response.marker_actions {
            let _ = self.session.apply_boxed(action.into_command(sequence));
        }
    }

    /// Applies the track-header gestures of one timeline frame.
    ///
    /// A gain drag raises one action a frame so the mixer follows the pointer,
    /// and they all belong to one entry in the undo stack; the panel brackets
    /// them with the group it opens and commits.
    fn apply_track_actions(&mut self, sequence: SequenceId, response: &TimelineResponse) {
        if response.actions.is_empty() && !response.actions_commit {
            return;
        }
        if let Some(label) = response.actions_begin.clone()
            && self.session.begin_group(label).is_err()
        {
            return;
        }
        for action in response.actions.clone() {
            let _ = self.session.apply_boxed(action.into_command(sequence));
        }
        if response.actions_commit {
            let _ = self.session.commit_group();
        }
    }

    /// Applies the inspector's gesture, as one entry in the undo stack.
    fn apply_inspector(&mut self, response: InspectorResponse) {
        if let Some(label) = response.begin
            && self.session.begin_group(label).is_err()
        {
            return;
        }
        for command in response.commands {
            let _ = self.session.apply(command);
        }
        for edit in response.effects {
            let _ = match edit {
                EffectEdit::Add(command) => self.session.apply(command),
                EffectEdit::Move(command) => self.session.apply(command),
                EffectEdit::Remove(command) => self.session.apply(command),
                EffectEdit::SetParam(command) => self.session.apply(command),
            };
        }
        if response.commit {
            let _ = self.session.commit_group();
        }
    }

    /// Draws the sequence tab strip and applies what it asked for.
    ///
    /// Switching tabs is view state and changes nothing in the project;
    /// creating, renaming and deleting a sequence are each one command.
    fn sequence_tabs_ui(&mut self, ui: &mut egui::Ui) {
        let project = self.session.project_arc();
        let Some(action) = self.tabs.ui(ui, &project.sequences) else {
            return;
        };
        match action {
            SequenceTabAction::Switch(sequence) => {
                if let Err(error) = self.tabs.switch_to(
                    &project.sequences,
                    sequence,
                    &mut self.timeline,
                    &mut self.viewer.state,
                ) {
                    log::warn!("sequence tabs: [{}] {}", error.code, error.message);
                }
            }
            SequenceTabAction::Delete(sequence) => {
                // The last tab cannot be closed: a project always has a
                // sequence to edit. The command itself stays permissive
                // because it is also the inverse of creating the first one.
                match SequenceTabs::check_delete(&project.sequences, sequence) {
                    Ok(()) => {
                        let _ = self.session.apply(DeleteSequence::new(sequence));
                    }
                    Err(error) => log::warn!("sequence tabs: [{}] {}", error.code, error.message),
                }
            }
            other => {
                if let Some(command) = other.into_command() {
                    let _ = self.session.apply_boxed(command);
                }
            }
        }
        self.sync_project();
    }

    /// Draws the File menu and runs what it chose.
    fn file_menu_ui(&mut self, ui: &mut egui::Ui) {
        let mut restored = None;
        let mut open = None;
        let mut save = false;
        let mut save_as = None;
        let saveable = self.session.project_file().is_some();
        ui.menu_button("File", |ui| {
            if ui.button(OPEN_LABEL).clicked() {
                open = pick_project_file();
                ui.close();
            }
            if ui
                .add_enabled(saveable, egui::Button::new(SAVE_LABEL))
                .clicked()
            {
                save = true;
                ui.close();
            }
            if ui.button(SAVE_AS_LABEL).clicked() {
                save_as = pick_project_destination();
                ui.close();
            }
            ui.menu_button(crate::recovery::MENU_TITLE, |ui| {
                restored = self.snapshots.ui(ui);
            });
        });
        if let Some(path) = open
            && let Err(error) = self.open_project(&path)
        {
            log::warn!("open: [{}] {}", error.code, error.message);
        }
        if save {
            let _ = self.save_project();
        }
        if let Some(path) = save_as {
            let _ = self.save_project_as(&path);
        }
        self.apply_recovery(restored);
    }

    /// Draws the Edit menu over the engine's history and runs what it chose.
    fn edit_menu_ui(&mut self, ui: &mut egui::Ui) {
        let list = self
            .session
            .history()
            .as_ref()
            .map(HistoryList::from_summary)
            .unwrap_or_default();
        if let Some(action) = edit_menu_ui(ui, &list, &self.keymap.map) {
            self.perform_history(action);
        }
    }

    /// How many frames have been painted since startup.
    pub fn frames_painted(&self) -> u32 {
        self.frames_painted
    }

    /// Whether this run should end now, and why.
    ///
    /// `--smoke-test` counts frames and the window smoke run counts seconds;
    /// a run given both ends on whichever arrives first.
    fn unattended_close_reason(&self) -> Option<String> {
        if self
            .options
            .smoke_frames
            .is_some_and(|target| self.frames_painted >= target)
        {
            return Some(format!("smoke test painted {} frames", self.frames_painted));
        }
        let hold = self.options.hold?;
        let elapsed = self.started.elapsed();
        (elapsed >= hold).then(|| format!("window held for {:.1}s", elapsed.as_secs_f32()))
    }

    /// How the project named on the command line loaded.
    pub const fn project_state(&self) -> ProjectState {
        self.project_state
    }

    /// Whether every window this run asked for has painted a frame.
    ///
    /// The pop-out paints on its own pass, so "the app is up" is not the
    /// editor window alone: a screenshot taken before the second window has a
    /// picture would photograph an empty rectangle.
    pub fn windows_are_up(&self) -> bool {
        self.frames_painted > 0
            && (!self.popout.is_open() || self.popout.shared().frames_painted() > 0)
            // And the picture in them is the one the playhead is on. A window
            // photographed before its decoders landed shows the bare canvas
            // and proves nothing about the media; a project whose clips are
            // offline, or which has none, settles immediately.
            && self.previews.settled()
    }

    /// Prints the ready line once every window has a picture.
    ///
    /// `scripts/ui-smoke.sh` waits for this line before it captures, so the
    /// wording and the fields are a contract; see [`UI_SMOKE_READY`].
    fn announce_ready(&mut self) {
        if self.announced_ready || !self.windows_are_up() || !self.command_api_settled() {
            return;
        }
        self.announced_ready = true;
        let stats = self.previews.stats();
        log::info!(
            "{UI_SMOKE_READY}: frames={} popout={} popout_frames={} project={} sequences={} \
             tracks={} revision={} command_api={} picture={} canvas={}",
            self.frames_painted,
            self.popout.is_open(),
            self.popout.shared().frames_painted(),
            self.project_state.label(),
            self.session.project().sequences.len(),
            self.sequence.tracks.len(),
            self.session.revision(),
            self.command_api_label(),
            stats.showing,
            self.canvas_label(),
        );
        for (clip, error) in self.previews.failures() {
            log::warn!(
                "clip {clip} has no preview picture: [{}] {}",
                error.code,
                error.message
            );
        }
    }

    /// What the compositor's canvas actually holds, for the ready line.
    ///
    /// `lit` when the canvas carries a pixel that is not black and `black`
    /// when it does not — which is what an unattended run's screenshot is
    /// worth looking at for. The canvas is read back off the GPU, which is
    /// megabytes of copy, so it is only asked for on a run that exists to be
    /// photographed: one told how long to hold its windows up, or how many
    /// frames to paint.
    fn canvas_label(&self) -> &'static str {
        if self.options.hold.is_none() && self.options.smoke_frames.is_none() {
            return "unread";
        }
        let canvas = self.compositor.read_rgba();
        if canvas.chunks_exact(4).any(|pixel| pixel[..3] != [0, 0, 0]) {
            "lit"
        } else {
            "black"
        }
    }

    /// Whether the Command API has finished starting, one way or the other.
    ///
    /// The ready line waits for it: a CI step that connects the moment the
    /// line appears would otherwise race the bind.
    fn command_api_settled(&self) -> bool {
        self.command_api
            .as_ref()
            .is_none_or(|api| api.is_serving() || api.refusal().is_some())
    }

    /// What the ready line says about the agent surface: the address it is
    /// listening on, the code that refused it, or `off`.
    fn command_api_label(&self) -> String {
        match &self.command_api {
            None => "off".to_owned(),
            Some(api) => api.address().map_or_else(
                || {
                    api.refusal().map_or_else(
                        || "starting".to_owned(),
                        |error| format!("refused:{}", error.code),
                    )
                },
                Address::to_wire,
            ),
        }
    }
}

impl eframe::App for SubordinateApp {
    /// eframe's periodic save, and the one it makes on exit.
    ///
    /// The panel arrangement is ours to write rather than eframe storage's,
    /// so this is where a rearranged layout reaches `layout.json`. An
    /// unchanged layout writes nothing.
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        match self.layout.persist() {
            Ok(true) => log::debug!("panel layout saved"),
            Ok(false) => {}
            Err(error) => log::warn!("layout: [{}] {}", error.code, error.message),
        }
        match self.fullscreen.persist() {
            Ok(true) => log::debug!("fullscreen display saved"),
            Ok(false) => {}
            Err(error) => log::warn!("fullscreen: [{}] {}", error.code, error.message),
        }
    }

    /// Releases the Command API endpoint as the window closes, so a clean
    /// exit leaves no socket and no lock file behind for the next editor — or
    /// for a bridge — to find.
    fn on_exit(&mut self) {
        if let Some(api) = &mut self.command_api {
            api.shutdown();
            log::info!("the Command API endpoint has been released");
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // The adapter line stays in the bar because it is what makes a
        // startup problem obvious at a glance.
        ui.horizontal(|ui| {
            ui.heading("Subordinate");
            ui.label(format!(
                "{} - {}",
                self.render.backend_label(),
                self.render.describe()
            ));
            self.file_menu_ui(ui);
            self.edit_menu_ui(ui);
            // The picker lists whatever displays this platform lets the editor
            // describe, which is at least the one the editor is on.
            self.fullscreen.refresh_from_context(ui.ctx());
            let mut fullscreen_action = None;
            let popout_is_fullscreen = self.popout.is_fullscreen();
            ui.menu_button("View", |ui| {
                layout_menu_ui(ui, &mut self.layout);
                popout_menu_ui(ui, &mut self.popout);
                ui.separator();
                fullscreen_action =
                    monitor_picker_ui(ui, &mut self.fullscreen, popout_is_fullscreen);
            });
            self.apply_fullscreen(fullscreen_action);
            if ui.button("Hardware diagnostics").clicked() {
                self.diagnostics.open = !self.diagnostics.open;
            }
            if ui.button("Audio settings").clicked() {
                self.audio_settings.open = !self.audio_settings.open;
            }
            if ui.button("Keyboard shortcuts").clicked() {
                self.shortcuts_window.toggle();
            }
        });
        // The socket is collected before the session is polled, so a bind that
        // finished since the last frame is serving by the time anything reads
        // it, and a project opened last frame has taken its endpoint with it.
        if let Some(api) = &mut self.command_api {
            api.sync(&self.session);
        }
        // Anything the Command API, the MCP bridge or a plugin applied since
        // the last frame arrives here, so an edit made from outside the window
        // shows up in the panels without anyone telling them.
        if self.session.poll() {
            self.sync_project();
        }
        // The sequence tab strip sits between the menu bar and the dock: it is
        // which sequence is being edited, so it belongs to the window rather
        // than to any one panel.
        self.sequence_tabs_ui(ui);
        // The prompt is drawn over everything else, because it is the first
        // question an open asks.
        let answered = self.recovery.ui(ui.ctx());
        self.apply_recovery(answered);
        self.diagnostics.show(ui.ctx());
        self.apply_audio_settings(ui.ctx());
        self.shortcuts_window
            .show_with_problems(ui.ctx(), &self.keymap.map, &self.keymap.problems);

        // Where the playhead started this frame, so a drag on the scrub bar
        // or the timeline ruler can be heard (docs/PLAN.md §5.4).
        let playhead_was = self.viewer.state.playhead();

        // The map runs before any panel reads the keyboard, so a bound chord
        // is handled once, here, and never again by a panel further down.
        if self.apply_shortcuts(ui.ctx()) {
            self.needs_composite = true;
        }

        // The meters are read once a frame, straight out of the atomics the
        // audio callback stores into: two loads, no lock, and nothing the
        // callback has to wait for.
        let elapsed = ui.input(|input| input.stable_dt);
        self.viewer
            .update_master_meter(self.meters.master(), elapsed);

        // The clock runs after the keyboard, so a press this frame takes
        // effect on this frame's advance rather than the next one.
        // egui reports the frame delta as float seconds; that is the one
        // place a float enters, and it becomes whole nanoseconds before the
        // clock does any arithmetic with it.
        if self.run_transport(ui.ctx(), Duration::from_secs_f32(elapsed.max(0.0))) {
            self.needs_composite = true;
        }

        // The running export reports itself once a frame: the bar, the
        // percentage and the ETA the panel shows are the job's own numbers.
        // Its events come from a worker thread, which egui has no reason to
        // wake for, so a running export asks for the next frame itself.
        self.poll_export();
        if self.export.runner.is_running() {
            ui.ctx().request_repaint_after(EXPORT_POLL_INTERVAL);
        }

        // The imports and the relink search report themselves the same way:
        // their workers finish between frames, so a window with either still
        // running asks for the next frame itself rather than waiting for the
        // user to move the pointer.
        if self.poll_media() {
            ui.ctx().request_repaint_after(EXPORT_POLL_INTERVAL);
        }
        self.relink_ui(ui.ctx());

        let preview = self.composite(ui.ctx());
        // The pop-out runs before the dock, so the panel knows on this frame
        // whether the picture is its to paint.
        if self.run_popout(ui.ctx(), preview) {
            self.needs_composite = true;
        }
        if self.dock_ui(ui, preview) {
            self.needs_composite = true;
        }

        // Moving the playhead by hand — dragging it, or stepping it — plays a
        // short grain of the mix around where it landed. Playback itself is
        // not a scrub: it already has the stream.
        let playhead = self.viewer.state.playhead();
        if playhead != playhead_was && !self.scheduler.is_playing() {
            self.scrub_audio(playhead);
        }

        self.frames_painted = self.frames_painted.saturating_add(1);

        if self.options.is_unattended() {
            let ctx = ui.ctx();
            // Nothing is animating, so ask for the next frame explicitly.
            ctx.request_repaint();
            self.announce_ready();
            if !self.closing
                && let Some(reason) = self.unattended_close_reason()
            {
                self.closing = true;
                log::info!("{reason}; closing");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

/// wgpu configuration for eframe: our adapter preference, and the backend
/// logged as soon as it is known.
/// The project file the user picked to open, if they picked one.
///
/// Returns `None` on a machine with no portal or dialog to show, which is
/// every headless test: the caller then does nothing, rather than blocking.
#[must_use]
pub fn pick_project_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Subordinate project", &[PROJECT_EXTENSION])
        .pick_file()
}

/// The export side of the window, in one place.
///
/// The panel holds the choices, the library resolves the chosen preset back
/// to settings when an export starts, and the runner owns whatever job is
/// running — which is what the panel's progress bar, ETA and Cancel button
/// reach.
struct ExportHost {
    /// The export panel: preset, range, output file and render progress.
    panel: ExportPanel,
    /// The presets the panel offers, kept so a started export can be resolved
    /// back to the settings its preset asks for.
    presets: PresetLibrary,
    /// The export running now, if one is.
    runner: ExportRunner,
}

impl ExportHost {
    /// The export side of a fresh window, with the user's presets loaded.
    ///
    /// A preset file that will not parse is shown in the panel rather than
    /// swallowed: the built-ins are still there, so the editor still exports.
    fn new() -> Self {
        let mut panel = ExportPanel::new();
        let presets = match PresetLibrary::load() {
            Ok(library) => library,
            Err(error) => {
                panel.add_problem(error);
                PresetLibrary::builtin()
            }
        };
        panel.set_library(&presets);
        panel.set_unavailable(Some(NO_RENDERER_REASON));
        Self {
            panel,
            presets,
            runner: ExportRunner::new(),
        }
    }
}

/// Where the user wants the exported file written, if they chose somewhere.
#[must_use]
pub fn pick_export_destination(current: &std::path::Path) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(name) = current.file_name().and_then(|name| name.to_str()) {
        dialog = dialog.set_file_name(name);
    }
    if let Some(dir) = current.parent().filter(|dir| dir.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    dialog.save_file()
}

/// Where the user wants the project written, if they chose somewhere.
#[must_use]
pub fn pick_project_destination() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Subordinate project", &[PROJECT_EXTENSION])
        .save_file()
}

/// The `SubError` a render failure at startup reports.
///
/// `RenderError` carries its own stable code; this only lifts it into the
/// shared error type so the whole of startup fails the same way.
fn render_error(error: &RenderError) -> SubError {
    SubError::wrap(
        sub_core::ErrorCode::from_static(error.code()),
        "the editor could not start",
        error,
    )
}

/// The edit an action asks the bin's selection for, when it asks for one.
#[must_use]
pub const fn edit_mode_for(action: Action) -> Option<EditMode> {
    match action {
        Action::InsertAtPlayhead => Some(EditMode::Insert),
        Action::OverwriteAtPlayhead => Some(EditMode::Overwrite),
        _ => None,
    }
}

fn wgpu_configuration() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut configuration = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut configuration.wgpu_setup {
        setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, _surface| {
            for adapter in adapters {
                log::debug!("adapter found: {}", describe_adapter(&adapter.get_info()));
            }
            let adapter = select_adapter(adapters).ok_or_else(|| {
                RenderError::NoAdapter {
                    backends: "eframe defaults".to_owned(),
                }
                .to_string()
            })?;
            log::info!("adapter chosen: {}", describe_adapter(&adapter.get_info()));
            Ok(adapter.clone())
        }));
    }
    configuration
}

/// The pop-out viewer as this run's options and saved settings leave it.
///
/// `monitor` is the display the user last chose to go fullscreen on, so the
/// choice is in force from the first frame rather than from the first visit to
/// the View menu.
fn startup_popout(options: &AppOptions, monitor: Option<usize>) -> PopoutViewer {
    let mut popout = PopoutViewer::new();
    if let Some(position) = options.popout_position {
        popout.set_position(position);
    }
    popout.set_monitor(monitor);
    if options.open_popout {
        popout.open();
    }
    popout
}

/// Native options for the main window.
fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Subordinate")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0]),
        wgpu_options: wgpu_configuration(),
        ..Default::default()
    }
}

/// Launch the editor window and run until it closes.
///
/// # Errors
///
/// Whatever eframe reports: no display, no usable adapter, or a window that
/// could not be created.
pub fn run(options: AppOptions) -> eframe::Result {
    eframe::run_native(
        "subordinate",
        native_options(),
        Box::new(move |cc| Ok(Box::new(SubordinateApp::new(cc, options)?))),
    )
}

/// The output stage for a sequence at `sample_rate`, closed.
///
/// The factory it carries builds a fresh mixer every time a stream opens,
/// because a reopened stream needs a mixer paired with a fresh control half.
/// That control half is handed back through `control` so the transport can
/// seek the mixer to the playhead: it is the same clock the picture follows.
/// The graph itself is still empty — feeding it the sequence's clips is the
/// next task — so what plays is silence at the right position.
fn audio_output(
    sample_rate: u32,
    meters: Arc<MeterBank>,
    control: Rc<RefCell<Option<MixerControl>>>,
    scrub_control: Rc<RefCell<Option<ScrubControl>>>,
    scrub_settings: Rc<Cell<ScrubSettings>>,
) -> AudioOutput<CpalBackend> {
    AudioOutput::new(
        CpalBackend::new(),
        OutputOptions::default(),
        Box::new(move || {
            let graph = MixGraphBuilder::new(sample_rate, 2).build()?;
            let (fresh, mixer) = mixer(graph, MixerConfig::default())?;
            *control.borrow_mut() = Some(fresh);
            let (scrubber, player) = scrub(scrub_settings.get(), sample_rate)?;
            *scrub_control.borrow_mut() = Some(scrubber);
            Ok(mixer.with_meters(Arc::clone(&meters)).with_scrub(player))
        }),
    )
}

/// How long a grain of these settings lasts, as a wall-clock duration.
///
/// The grain itself is an exact [`RationalTime`]; this is only how long the
/// output stage is held open for it, which is wall time by nature.
fn grain_duration(settings: ScrubSettings) -> Duration {
    let (numerator, denominator) = settings.grain.as_seconds_fraction();
    if denominator <= 0 || numerator <= 0 {
        return Duration::ZERO;
    }
    let nanos = numerator.saturating_mul(1_000_000_000) / denominator;
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::{AppOptions, ProjectState, UI_SMOKE_READY};
    use std::time::Duration;

    #[test]
    fn options_default_to_a_normal_run() {
        let options = AppOptions::default();
        assert_eq!(options.smoke_frames, None);
        assert_eq!(options.hold, None);
        assert!(!options.open_popout);
        assert_eq!(options.popout_position, None);
        assert!(
            !options.is_unattended(),
            "a normal run waits for the user, not for a clock"
        );
    }

    #[test]
    fn either_ci_run_paints_on_its_own() {
        let counted = AppOptions {
            smoke_frames: Some(3),
            ..AppOptions::default()
        };
        assert!(counted.is_unattended());
        let held = AppOptions {
            hold: Some(Duration::from_secs(5)),
            ..AppOptions::default()
        };
        assert!(held.is_unattended());
    }

    #[test]
    fn the_ready_line_reports_the_project() {
        // scripts/ui-smoke.sh greps for both of these; a rename here without
        // one there is a CI job that waits for a line that never comes.
        assert_eq!(UI_SMOKE_READY, "ui-smoke ready");
        assert_eq!(ProjectState::Loaded.label(), "loaded");
        assert_eq!(ProjectState::Failed.label(), "failed");
        assert_eq!(ProjectState::None.label(), "none");
    }

    #[test]
    fn the_window_is_configured_before_any_gpu_work() {
        // Building the options must not need a display or an adapter; only
        // `run` may touch either.
        let options = super::native_options();
        assert_eq!(
            options.viewport.title.as_deref(),
            Some("Subordinate"),
            "the window should be titled"
        );
    }
}
