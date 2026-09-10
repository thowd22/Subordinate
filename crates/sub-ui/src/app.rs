//! The eframe application and its wgpu setup.
//!
//! eframe runs on the wgpu backend so the device it creates for egui is the
//! very device the compositor draws preview frames with (docs/PLAN.md §3).
//! [`SubordinateApp::new`] lifts that device, queue and adapter out of
//! eframe's `RenderState` into a [`RenderContext`], which is what every other
//! crate sees.

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use sub_core::SubError;
use sub_edit::playback::{PlaybackScheduler, ShuttleSpeed};
use sub_model::Project;
use sub_model::sequence::{Resolution, Sequence, SequenceSettings};
use sub_render::{
    Compositor, RenderContext, RenderError, ResolvedClip, SourceFrame, describe_adapter,
    select_adapter,
};
use sub_time::RationalTime;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sub_audio::mixer::{MixGraphBuilder, MixerConfig, MixerControl, mixer};
use sub_audio::{AudioOutput, CpalBackend, MeterBank, OutputOptions};

use crate::audio_settings::{AudioSettingsAction, AudioSettingsPanel};
use crate::diagnostics::DiagnosticsPanel;
use crate::dock::{DockLayout, Panel, layout_menu_ui};
use crate::keymap::LoadedKeymap;
use crate::media_bin::MediaBinPanel;
use crate::popout::{PopoutViewer, popout_menu_ui};
use crate::recovery::{RecoveryOutcome, RecoveryPrompt, SnapshotMenu};
use crate::shortcuts::{Action, ShortcutMap, ShortcutsWindow};
use crate::timeline_panel::TimelinePanel;
use crate::viewer::{TransportAction, ViewerAction, ViewerFrame, ViewerPanel};

/// How many tracks the shared meter bank has room for. A sequence with more
/// audio tracks than this still plays; the tracks past it are unmetered.
const METERED_TRACKS: usize = 64;

/// The line the window smoke run prints once every window has a picture.
///
/// CI waits for it before it takes a screenshot, so it is part of the
/// contract with `scripts/ui-smoke.sh` (TASK-123) rather than a stray log
/// line. Anything after the colon is diagnostics.
pub const UI_SMOKE_READY: &str = "ui-smoke ready";

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
            ..Self::default()
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
    /// The project the bin and the timeline show. Loading a project replaces
    /// it; until then it is empty, as the sequence is.
    project: Project,
    /// The media bin panel.
    media_bin: MediaBinPanel,
    /// The timeline panel.
    timeline: TimelinePanel,
    /// Where the panels are docked, as read from the user's `layout.json`.
    layout: DockLayout,
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
    /// The file the open project came from, once one has been opened. The
    /// autosave history hangs off it.
    project_file: Option<PathBuf>,
    /// The prompt an open shows when an autosave is ahead of the file.
    recovery: RecoveryPrompt,
    /// The restore list in the File menu.
    snapshots: SnapshotMenu,
}

impl SubordinateApp {
    /// Build the app from eframe's creation context.
    ///
    /// # Errors
    ///
    /// [`RenderError::MissingRenderState`] when eframe was built without its
    /// wgpu backend, in which case there is no device to share.
    pub fn new(cc: &eframe::CreationContext<'_>, options: AppOptions) -> Result<Self, RenderError> {
        let state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or(RenderError::MissingRenderState)?;
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
        let meters = Arc::new(MeterBank::new(METERED_TRACKS));
        let audio_control = Rc::new(RefCell::new(None));
        let audio = audio_output(
            sequence.settings.sample_rate,
            Arc::clone(&meters),
            Rc::clone(&audio_control),
        );
        let mut popout = PopoutViewer::new();
        if let Some(position) = options.popout_position {
            popout.set_position(position);
        }
        if options.open_popout {
            popout.open();
        }
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
            project: Project::new("Untitled"),
            media_bin: MediaBinPanel::new(),
            timeline,
            layout: layout.layout,
            preview: None,
            needs_composite: true,
            keymap,
            shortcuts_window: ShortcutsWindow::new(),
            project_file: None,
            recovery: RecoveryPrompt::new(),
            snapshots: SnapshotMenu::new(),
        };
        if let Some(path) = startup_project {
            app.project_state = match app.open_project(&path) {
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
        Ok(app)
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
        let text = std::fs::read_to_string(path).map_err(|err| {
            SubError::wrap(
                crate::codes::PROJECT_UNREADABLE,
                "a project cannot be read",
                &err,
            )
            .with_detail("path", path.display().to_string())
        })?;
        let project = sub_model::json::from_json(&text)?;
        self.adopt_project(project);
        self.project_file = Some(path.to_path_buf());
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

    /// The file the open project came from, once one has been opened.
    pub fn project_file(&self) -> Option<&Path> {
        self.project_file.as_deref()
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
    /// the state instead of going through the history as a command.
    pub fn adopt_project(&mut self, project: Project) {
        let sequence = project
            .sequences
            .first()
            .cloned()
            .unwrap_or_else(|| Sequence::new("Sequence", SequenceSettings::default()));
        self.compositor = Compositor::for_sequence(self.render.clone(), &sequence);
        self.viewer = ViewerPanel::for_sequence(&sequence);
        self.scheduler = PlaybackScheduler::for_sequence(&sequence);
        self.timeline = TimelinePanel::new(sequence.settings.frame_rate);
        self.sequence = sequence;
        self.project = project;
        self.needs_composite = true;
    }

    /// Applies what the recovery prompt or the restore list handed back.
    ///
    /// A failure is logged and shown by the widget that raised it; it never
    /// disturbs the project already open.
    fn apply_recovery(&mut self, outcome: Option<RecoveryOutcome>) {
        match outcome {
            Some(RecoveryOutcome::Recovered(project)) => {
                self.adopt_project(*project);
                if let Err(error) = self.snapshots.refresh() {
                    log::warn!("autosave history: [{}] {}", error.code, error.message);
                }
            }
            Some(RecoveryOutcome::Discarded) => {
                if let Err(error) = self.snapshots.refresh() {
                    log::warn!("autosave history: [{}] {}", error.code, error.message);
                }
            }
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
        match self.audio_settings.show(ctx) {
            AudioSettingsAction::None => {}
            AudioSettingsAction::Rescan => self.audio_settings.refresh(),
            AudioSettingsAction::SelectDevice(device_id) => {
                let result = self.audio.select_device(device_id.as_deref());
                self.audio_settings.set_error(result.err());
                self.audio_settings
                    .set_selected(self.audio.selected_device());
            }
        }
    }

    /// The viewer panel, which owns the playhead.
    pub fn viewer(&mut self) -> &mut ViewerPanel {
        &mut self.viewer
    }

    /// The viewer's pop-out window.
    pub fn popout(&mut self) -> &mut PopoutViewer {
        &mut self.popout
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
    /// Actions whose panels do not exist yet (playback, marking, editing) are
    /// logged and dropped; they are wired up by the tasks that add them.
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
            } else {
                log::debug!("shortcut {} is not wired up yet", action.id());
            }
        }
        moved
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
            self.stop_audio();
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

    /// Composites the sequence at the playhead, if the playhead has moved,
    /// and returns the picture the viewer should sample.
    ///
    /// The playback clock now moves the playhead, but nothing feeds decoded
    /// pictures to the compositor yet: the frame source stays empty until the
    /// decode path is wired to it, so every clip resolves to "no picture
    /// ready" and the composite is the bare black canvas. The output texture
    /// is registered with egui once and re-registered only when the canvas
    /// size changes, because [`Compositor::render`] otherwise keeps drawing
    /// into the same texture.
    fn composite(&mut self) -> ViewerFrame {
        if self.needs_composite {
            let mut empty = |_: &ResolvedClip<'_>| -> Option<SourceFrame> { None };
            self.compositor
                .render(&self.sequence, self.viewer.state.playhead(), &mut empty);
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
    /// exactly as it did. The inspector and export panels have no widgets of
    /// their own yet (TASK-62 brings the export one), so their tabs say so
    /// rather than showing an empty rectangle.
    fn dock_ui(&mut self, ui: &mut egui::Ui, preview: ViewerFrame) -> bool {
        // Destructured so each panel body borrows the fields it draws with
        // while the dock borrows the layout.
        let Self {
            layout,
            viewer,
            media_bin,
            timeline,
            project,
            sequence,
            ..
        } = self;
        // The engine's revision counter reaches the app with the engine
        // handle; until then the sequence never changes, so one revision is
        // the whole story and the sync is a no-op after the first frame.
        timeline.sync(sequence, 0);
        // The viewer owns the playhead; the timeline draws it and can ask for
        // a new one, so it is handed the current value before it paints.
        timeline.set_playhead(viewer.state.playhead());
        let mut moved = false;
        layout.ui(ui, |ui, panel| match panel {
            Panel::Viewer => moved |= viewer.ui(ui, Some(preview)),
            Panel::MediaBin => {
                for action in media_bin.ui(ui, project) {
                    // The bin's actions become commands once the app owns an
                    // engine handle; until then they are logged rather than
                    // silently swallowed.
                    log::debug!("media bin action is not wired up yet: {action:?}");
                }
            }
            Panel::Timeline => {
                let response = timeline.ui(ui, project, sequence);
                if let Some(time) = response.seek {
                    // Clicking or scrubbing the ruler moves the playhead,
                    // which is view state rather than part of the edit, so it
                    // is a seek and not a Command.
                    moved |= viewer.state.seek_to(time);
                }
                for action in response.actions {
                    log::debug!("timeline action is not wired up yet: {action:?}");
                }
            }
            Panel::Inspector => {
                ui.label("The inspector arrives with the parameter panel.");
            }
            Panel::Export => {
                ui.label("The export panel arrives with TASK-62.");
            }
        });
        moved
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
    }

    /// Prints the ready line once every window has a picture.
    ///
    /// `scripts/ui-smoke.sh` waits for this line before it captures, so the
    /// wording and the fields are a contract; see [`UI_SMOKE_READY`].
    fn announce_ready(&mut self) {
        if self.announced_ready || !self.windows_are_up() {
            return;
        }
        self.announced_ready = true;
        log::info!(
            "{UI_SMOKE_READY}: frames={} popout={} popout_frames={} project={}",
            self.frames_painted,
            self.popout.is_open(),
            self.popout.shared().frames_painted(),
            self.project_state.label()
        );
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
            let mut restored = None;
            ui.menu_button("File", |ui| {
                ui.menu_button(crate::recovery::MENU_TITLE, |ui| {
                    restored = self.snapshots.ui(ui);
                });
            });
            self.apply_recovery(restored);
            ui.menu_button("View", |ui| {
                layout_menu_ui(ui, &mut self.layout);
                popout_menu_ui(ui, &mut self.popout);
            });
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
        // The prompt is drawn over everything else, because it is the first
        // question an open asks.
        let answered = self.recovery.ui(ui.ctx());
        self.apply_recovery(answered);
        self.diagnostics.show(ui.ctx());
        self.apply_audio_settings(ui.ctx());
        self.shortcuts_window
            .show_with_problems(ui.ctx(), &self.keymap.map, &self.keymap.problems);

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

        let preview = self.composite();
        // The pop-out runs before the dock, so the panel knows on this frame
        // whether the picture is its to paint.
        if self.run_popout(ui.ctx(), preview) {
            self.needs_composite = true;
        }
        if self.dock_ui(ui, preview) {
            self.needs_composite = true;
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
) -> AudioOutput<CpalBackend> {
    AudioOutput::new(
        CpalBackend::new(),
        OutputOptions::default(),
        Box::new(move || {
            let graph = MixGraphBuilder::new(sample_rate, 2).build()?;
            let (fresh, mixer) = mixer(graph, MixerConfig::default())?;
            *control.borrow_mut() = Some(fresh);
            Ok(mixer.with_meters(Arc::clone(&meters)))
        }),
    )
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
