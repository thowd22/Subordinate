//! The egui side: one converted texture, painted in the main window and in a
//! pop-out viewport at the same time.
//!
//! The point being de-risked (docs/PLAN.md §11 step 3) is that the pop-out
//! costs nothing extra. eframe hands the app the very `wgpu::Device` egui
//! paints with, so the converted RGB texture is registered with egui *once*
//! and both viewports reference the same [`egui::TextureId`]. No second
//! upload, no readback, no copy — moving the pop-out to another monitor is
//! then a window-manager problem, not a rendering one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;
use sub_render::{RenderContext, RenderError};

use crate::decode::{Decoder, decoder_kind};
use crate::frames::{Durations, FrameSlot};
use crate::nv12::Nv12Converter;

/// What the pop-out viewport needs in order to paint, shared with the closure
/// eframe keeps alive across frames.
#[derive(Debug, Default)]
struct SharedPreview {
    texture: Mutex<Option<(egui::TextureId, [f32; 2])>>,
    frames_painted: AtomicU64,
}

/// How the spike was asked to run.
#[derive(Debug, Clone)]
pub struct PlayOptions {
    /// File to decode, as a URI.
    pub uri: String,
    /// Open the pop-out viewport as soon as the window appears.
    pub pop_out: bool,
    /// Close after this many painted frames, for an unattended run.
    pub max_frames: Option<u64>,
}

/// The spike window.
pub struct SpikeApp {
    render: RenderContext,
    renderer: Arc<egui::mutex::RwLock<eframe::egui_wgpu::Renderer>>,
    slot: Arc<FrameSlot>,
    decoder: Decoder,
    converter: Option<Nv12Converter>,
    texture: Option<egui::TextureId>,
    shared: Arc<SharedPreview>,
    options: PlayOptions,
    pop_out: bool,
    frames_shown: u64,
    upload: Durations,
    latency: Durations,
    error: Option<String>,
}

impl SpikeApp {
    /// Start decoding and build the window state.
    ///
    /// # Errors
    ///
    /// [`RenderError::MissingRenderState`] when eframe has no wgpu state, so
    /// there is no device to share; anything the decoder reports is surfaced
    /// in the window instead of failing startup.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        options: PlayOptions,
        slot: Arc<FrameSlot>,
        decoder: Decoder,
    ) -> Result<Self, RenderError> {
        let state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or(RenderError::MissingRenderState)?;
        let render = RenderContext::new(
            state.device.clone(),
            state.queue.clone(),
            state.adapter.get_info(),
        );
        tracing::info!(adapter = %render.describe(), "spike render device ready");
        Ok(Self {
            render,
            renderer: Arc::clone(&state.renderer),
            slot,
            decoder,
            converter: None,
            texture: None,
            shared: Arc::new(SharedPreview::default()),
            pop_out: options.pop_out,
            options,
            frames_shown: 0,
            upload: Durations::new(),
            latency: Durations::new(),
            error: None,
        })
    }

    /// The measurements taken so far: upload cost and decode-to-display
    /// latency, both in nanoseconds.
    pub fn measurements(&self) -> (&Durations, &Durations) {
        (&self.upload, &self.latency)
    }

    /// Take the newest decoded frame, upload it and convert it.
    fn consume_newest_frame(&mut self) {
        let Some(frame) = self.slot.take() else {
            return;
        };
        let converter = match &self.converter {
            Some(converter) if converter.geometry() == frame.geometry => converter,
            _ => {
                // First frame, or the source changed size: build the textures
                // once and register the output with egui once.
                let converter = Nv12Converter::new(self.render.device(), frame.geometry);
                let view = converter
                    .output()
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let id = self.renderer.write().register_native_texture(
                    self.render.device(),
                    &view,
                    wgpu::FilterMode::Linear,
                );
                self.texture = Some(id);
                self.converter = Some(converter);
                self.converter.as_ref().unwrap_or_else(|| unreachable!())
            }
        };

        let started = std::time::Instant::now();
        match converter.submit_frame(
            self.render.device(),
            self.render.queue(),
            &frame.y,
            &frame.uv,
        ) {
            Ok(_index) => {
                self.upload
                    .push_nanos(elapsed_nanos(started, std::time::Instant::now()));
                self.latency
                    .push_nanos(elapsed_nanos(frame.arrived, std::time::Instant::now()));
                self.frames_shown += 1;
                // An unattended run has no window to read, so the same status
                // the window shows is logged every 30 frames.
                if self.frames_shown.is_multiple_of(30) {
                    tracing::info!("{}", self.status());
                }
            }
            Err(error) => self.error = Some(format!("[{}] {}", error.code, error.message)),
        }

        if let (Some(id), Some(converter)) = (self.texture, self.converter.as_ref())
            && let Ok(mut shared) = self.shared.texture.lock()
        {
            let size = [
                pixels_as_f32(converter.geometry().width()),
                pixels_as_f32(converter.geometry().height()),
            ];
            *shared = Some((id, size));
        }
    }

    /// Paint the preview image, or a placeholder while nothing has decoded.
    fn show_preview(ui: &mut egui::Ui, texture: Option<(egui::TextureId, [f32; 2])>) {
        match texture {
            Some((id, size)) => {
                let available = ui.available_size();
                let scale = (available.x / size[0]).min(available.y / size[1]).max(0.01);
                ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(
                    id,
                    egui::vec2(size[0] * scale, size[1] * scale),
                )));
            }
            None => {
                ui.centered_and_justified(|ui| ui.label("waiting for the first decoded frame"));
            }
        }
    }

    /// The status line both the window and the findings quote.
    fn status(&self) -> String {
        let decoder = self.decoder.chosen_decoder().unwrap_or_default();
        let kind = if decoder.is_empty() {
            "not negotiated yet".to_owned()
        } else {
            format!("{decoder} ({})", decoder_kind(&decoder).label())
        };
        format!(
            "decoder {kind} | promoted [{}] | adapter {} | shown {} | delivered {} | dropped {}\nupload {}\nlatency {}",
            self.decoder.preferred_decoders().join(", "),
            self.render.describe(),
            self.frames_shown,
            self.decoder.stats().delivered(),
            self.decoder.stats().dropped(),
            self.upload.summary_micros(),
            self.latency.summary_micros(),
        )
    }

    /// Open or update the second window.
    fn show_pop_out(&self, ctx: &egui::Context) {
        let shared = Arc::clone(&self.shared);
        ctx.show_viewport_deferred(
            egui::ViewportId::from_hash_of("spike-popout"),
            egui::ViewportBuilder::default()
                .with_title("Subordinate viewer (pop-out)")
                .with_inner_size([960.0, 540.0]),
            move |ctx, _class| {
                let texture = shared.texture.lock().ok().and_then(|texture| *texture);
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        SpikeApp::show_preview(ui, texture);
                    });
                shared.frames_painted.fetch_add(1, Ordering::Relaxed);
            },
        );
    }
}

impl eframe::App for SpikeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.error.is_none()
            && let Some(error) = self.decoder.take_error()
        {
            self.error = Some(format!("[{}] {}", error.code, error.message));
        }
        self.consume_newest_frame();

        ui.heading("TASK-7 spike: NV12 -> wgpu -> egui");
        ui.label(self.status());
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::RED, error);
        }
        ui.checkbox(&mut self.pop_out, "pop out the viewer into a second window");
        ui.label(format!(
            "pop-out frames painted: {}",
            self.shared.frames_painted.load(Ordering::Relaxed)
        ));
        ui.separator();

        let texture = self.shared.texture.lock().ok().and_then(|texture| *texture);
        Self::show_preview(ui, texture);

        if self.pop_out {
            self.show_pop_out(ui.ctx());
        }

        // Decode runs on its own thread; ask for the next paint unconditionally
        // so a newly arrived frame is never left waiting in the slot.
        ui.ctx().request_repaint();

        if let Some(limit) = self.options.max_frames
            && self.frames_shown >= limit
        {
            tracing::info!("final: {}", self.status());
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// A pixel count as the `f32` egui lays out in.
///
/// Pictures are never wider than 65 535 pixels, so this is exact; going
/// through `u16` is what makes that provable rather than assumed.
fn pixels_as_f32(pixels: u32) -> f32 {
    f32::from(u16::try_from(pixels).unwrap_or(u16::MAX))
}

/// Nanoseconds between two instants.
fn elapsed_nanos(start: std::time::Instant, end: std::time::Instant) -> u64 {
    u64::try_from(end.saturating_duration_since(start).as_nanos()).unwrap_or(u64::MAX)
}

/// Native window options for the spike.
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Subordinate NV12 spike")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{PlayOptions, native_options};

    #[test]
    fn the_window_is_configured_without_touching_a_gpu() {
        let options = native_options();
        assert_eq!(
            options.viewport.title.as_deref(),
            Some("Subordinate NV12 spike")
        );
    }

    #[test]
    fn play_options_carry_the_pop_out_choice() {
        let options = PlayOptions {
            uri: "file:///tmp/a.mp4".to_owned(),
            pop_out: true,
            max_frames: Some(120),
        };
        assert!(options.pop_out);
        assert_eq!(options.max_frames, Some(120));
        assert!(options.uri.starts_with("file://"));
    }
}
