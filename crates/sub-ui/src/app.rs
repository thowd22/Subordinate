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
use sub_edit::playback::PlaybackScheduler;
use sub_model::sequence::{Resolution, Sequence, SequenceSettings};
use sub_render::{
    Compositor, RenderContext, RenderError, ResolvedClip, SourceFrame, describe_adapter,
    select_adapter,
};

use std::sync::Arc;
use std::time::Duration;

use sub_audio::mixer::{MixGraphBuilder, MixerConfig, mixer};
use sub_audio::{AudioOutput, CpalBackend, MeterBank, OutputOptions};

use crate::audio_settings::{AudioSettingsAction, AudioSettingsPanel};
use crate::diagnostics::DiagnosticsPanel;
use crate::keymap::LoadedKeymap;
use crate::shortcuts::{Action, ShortcutMap, ShortcutsWindow};
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
    /// The playback clock behind J, K, L and the space bar. It owns the
    /// playhead while playback runs; the viewer owns it the rest of the time,
    /// and the two are synchronised once a frame.
    scheduler: PlaybackScheduler,
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
        let meters = Arc::new(MeterBank::new(METERED_TRACKS));
        let audio = audio_output(sequence.settings.sample_rate, Arc::clone(&meters));
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

    /// Runs the playback clock for this frame and returns whether the
    /// playhead moved.
    ///
    /// The clock is the master while it plays: it is advanced by the wall time
    /// egui reports for the frame, and whichever frame it lands on becomes the
    /// viewer's playhead. Any other move — a scrub, a frame step — is fed the
    /// other way, so playback resumes from wherever the user left the
    /// playhead. Frames the clock skips because a frame took too long are
    /// dropped and counted by the scheduler rather than slowing playback down.
    fn run_transport(&mut self, ctx: &egui::Context, elapsed: Duration) -> bool {
        self.scheduler.set_duration(self.viewer.state.duration());
        if !self.scheduler.is_playing() {
            self.scheduler.seek(self.viewer.state.playhead());
            return false;
        }
        if self.scheduler.position() != self.viewer.state.playhead() {
            // The user scrubbed or stepped while playing; carry on from there.
            self.scheduler.seek(self.viewer.state.playhead());
        }
        let moved = self
            .scheduler
            .advance(elapsed)
            .is_some_and(|tick| self.viewer.state.seek_to(tick.position));
        // Playback only looks like playback if the next frame is asked for.
        ctx.request_repaint();
        moved
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // The timeline, bin and inspector panels land here in later tasks;
        // the adapter line stays because it is what makes a startup problem
        // obvious at a glance.
        ui.horizontal(|ui| {
            ui.heading("Subordinate");
            ui.label(format!(
                "{} - {}",
                self.render.backend_label(),
                self.render.describe()
            ));
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
        if self.viewer.ui(ui, Some(preview)) {
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
/// Until the transport is wired up (TASK-50) that mixer plays an empty graph,
/// so the device the user picks here is remembered rather than opened.
fn audio_output(sample_rate: u32, meters: Arc<MeterBank>) -> AudioOutput<CpalBackend> {
    AudioOutput::new(
        CpalBackend::new(),
        OutputOptions::default(),
        Box::new(move || {
            let graph = MixGraphBuilder::new(sample_rate, 2).build()?;
            let (_control, mixer) = mixer(graph, MixerConfig::default())?;
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
