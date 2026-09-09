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
use sub_model::sequence::{Resolution, Sequence, SequenceSettings};
use sub_render::{
    Compositor, RenderContext, RenderError, ResolvedClip, SourceFrame, describe_adapter,
    select_adapter,
};

use crate::diagnostics::DiagnosticsPanel;
use crate::viewer::{ViewerFrame, ViewerPanel};

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
    /// The sequence being previewed. Loading a project replaces it; until
    /// then it is an empty sequence, which composites to black.
    sequence: Sequence,
    /// The compositor drawing that sequence at the playhead.
    compositor: Compositor,
    /// The viewer panel: picture, scrub bar and timecode.
    viewer: ViewerPanel,
    /// The compositor output as egui knows it, and the canvas it was
    /// registered at, so a resolution change re-registers rather than
    /// stretching a texture that no longer exists.
    preview: Option<(egui::TextureId, Resolution)>,
    /// Whether the playhead has moved since the last composite.
    needs_composite: bool,
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
        let sequence = Sequence::new("Sequence", SequenceSettings::default());
        let compositor = Compositor::for_sequence(render.clone(), &sequence);
        let viewer = ViewerPanel::for_sequence(&sequence);
        Ok(Self {
            render,
            render_state: state.clone(),
            options,
            frames_painted: 0,
            closing: false,
            diagnostics: DiagnosticsPanel::new(),
            sequence,
            compositor,
            viewer,
            preview: None,
            needs_composite: true,
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

    /// The viewer panel, which owns the playhead.
    pub fn viewer(&mut self) -> &mut ViewerPanel {
        &mut self.viewer
    }

    /// Composites the sequence at the playhead, if the playhead has moved,
    /// and returns the picture the viewer should sample.
    ///
    /// The frame source is empty until the playback scheduler (TASK-23)
    /// supplies decoded pictures, so today every clip resolves to "no picture
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
        });
        self.diagnostics.show(ui.ctx());

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
