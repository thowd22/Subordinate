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
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use sub_audio::mixer::{MixGraphBuilder, MixerConfig, MixerControl, mixer};
use sub_audio::{AudioOutput, CpalBackend, MeterBank, OutputOptions};

use crate::audio_settings::{AudioSettingsAction, AudioSettingsPanel};
use crate::diagnostics::DiagnosticsPanel;
use crate::dock::{DockLayout, Panel, layout_menu_ui};
use crate::keymap::LoadedKeymap;
use crate::media_bin::MediaBinPanel;
use crate::shortcuts::{Action, ShortcutMap, ShortcutsWindow};
use crate::timeline_panel::TimelinePanel;
use crate::viewer::{TransportAction, ViewerAction, ViewerFrame, ViewerPanel};

/// How many tracks the shared meter bank has room for. A sequence with more
/// audio tracks than this still plays; the tracks past it are unmetered.
const METERED_TRACKS: usize = 64;

/// Options for launching the application.
#[derive(Debug, Clone, Default)]
pub struct AppOptions {
    /// Close the window after this many painted frames.
    ///
    /// `None` runs normally. `Some(n)` is the CI smoke test: the app starts,
    /// paints an empty window `n` times on whatever adapter the machine has
    /// (a software one on a hosted runner) and exits.
    pub smoke_frames: Option<u32>,
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
        Ok(Self {
            render,
            render_state: state.clone(),
            options,
            frames_painted: 0,
            closing: false,
            diagnostics: DiagnosticsPanel::new(),
            audio_settings: AudioSettingsPanel::new(),
            audio,
            meters,
            sequence,
            compositor,
            viewer,
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
        })
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

    /// Whether this run should end now.
    fn smoke_test_is_done(&self) -> bool {
        self.options
            .smoke_frames
            .is_some_and(|target| self.frames_painted >= target)
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
            ui.menu_button("View", |ui| {
                layout_menu_ui(ui, &mut self.layout);
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
        if self.dock_ui(ui, preview) {
            self.needs_composite = true;
        }

        self.frames_painted = self.frames_painted.saturating_add(1);

        if self.options.smoke_frames.is_some() {
            let ctx = ui.ctx();
            // Nothing is animating, so ask for the next frame explicitly.
            ctx.request_repaint();
            if self.smoke_test_is_done() && !self.closing {
                self.closing = true;
                log::info!("smoke test painted {} frames; closing", self.frames_painted);
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
    use super::AppOptions;

    #[test]
    fn options_default_to_a_normal_run() {
        assert_eq!(AppOptions::default().smoke_frames, None);
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
