//! The audio settings panel: which output device plays the timeline, and how
//! that stream is doing (docs/PLAN.md §5.4).
//!
//! The panel holds no audio state of its own. It is given the device list and
//! the stream's diagnostics, and hands back an [`AudioSettingsAction`] saying
//! what the user asked for; the application applies that to its
//! [`sub_audio::AudioOutput`], which is what actually closes and reopens the
//! stream. Keeping the two apart is what lets the panel be tested without an
//! audio device and the switching be tested without a window.

use eframe::egui;

use sub_audio::scrub::{MAX_GRAIN_MS, MIN_GRAIN_MS, ScrubSettings};
use sub_audio::{OutputDeviceInfo, OutputDiagnostics};
use sub_core::{SubError, SubResult};

/// What the user asked the panel for this frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum AudioSettingsAction {
    /// Nothing was clicked.
    #[default]
    None,
    /// Scan the host for devices again.
    Rescan,
    /// Play on this device, or on the system default when `None`.
    SelectDevice(Option<String>),
    /// Scrub with these settings: whether dragging the playhead sounds at
    /// all, and how long each grain is (`sub_audio::scrub`).
    SetScrub(ScrubSettings),
}

/// The panel's state: the device list once it has been scanned, the device the
/// output is on, and the last snapshot of the stream.
#[derive(Debug, Default)]
pub struct AudioSettingsPanel {
    /// `None` until the application scans; then the list or the error the
    /// scan failed with, kept so a failure is shown rather than retried every
    /// frame.
    devices: Option<Result<Vec<OutputDeviceInfo>, SubError>>,
    /// The device the output is on, or `None` for the system default.
    selected: Option<String>,
    /// The last snapshot of the open stream, or `None` when none is open.
    diagnostics: Option<OutputDiagnostics>,
    /// Why the last device switch failed, when one did.
    last_error: Option<SubError>,
    /// What scrubbing does, as the application last told the panel.
    scrub: ScrubSettings,
    /// Whether the window is showing.
    pub open: bool,
}

impl AudioSettingsPanel {
    /// A closed panel that has not been given a device list yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the application should scan the host and call
    /// [`AudioSettingsPanel::set_devices`].
    ///
    /// True only when the panel is showing and has nothing to show, so a
    /// closed panel never touches the audio host.
    pub fn needs_devices(&self) -> bool {
        self.open && self.devices.is_none()
    }

    /// Gives the panel the result of a device scan.
    pub fn set_devices(&mut self, devices: SubResult<Vec<OutputDeviceInfo>>) {
        self.devices = Some(devices);
    }

    /// The devices the panel is showing, once it has been given any.
    pub fn devices(&self) -> Option<Result<&[OutputDeviceInfo], &SubError>> {
        self.devices
            .as_ref()
            .map(|devices| devices.as_ref().map(Vec::as_slice))
    }

    /// Forgets the device list so the next frame scans again.
    pub fn refresh(&mut self) {
        self.devices = None;
    }

    /// The device the output is on, or `None` for the system default.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Records the device the output actually ended up on.
    pub fn set_selected(&mut self, device_id: Option<&str>) {
        self.selected = device_id.map(str::to_owned);
    }

    /// Records the latest snapshot of the open stream, or `None` when the
    /// output is closed.
    pub fn set_diagnostics(&mut self, diagnostics: Option<OutputDiagnostics>) {
        self.diagnostics = diagnostics;
    }

    /// The snapshot the panel is showing.
    pub fn diagnostics(&self) -> Option<&OutputDiagnostics> {
        self.diagnostics.as_ref()
    }

    /// Records why a device switch failed, so the panel can say so instead of
    /// silently keeping the old device.
    pub fn set_error(&mut self, error: Option<SubError>) {
        self.last_error = error;
    }

    /// The error from the last failed switch.
    pub fn last_error(&self) -> Option<&SubError> {
        self.last_error.as_ref()
    }

    /// Tells the panel what scrubbing is actually set to, so the widgets show
    /// what the audio stage applied rather than what was asked for.
    pub fn set_scrub(&mut self, scrub: ScrubSettings) {
        self.scrub = scrub;
    }

    /// The scrub settings the panel is showing.
    pub fn scrub(&self) -> ScrubSettings {
        self.scrub
    }

    /// Draws the panel as a window, if it is open.
    pub fn show(&mut self, ctx: &egui::Context) -> AudioSettingsAction {
        if !self.open {
            return AudioSettingsAction::None;
        }
        let mut open = self.open;
        let mut action = AudioSettingsAction::None;
        egui::Window::new("Audio settings")
            .open(&mut open)
            .resizable(true)
            .default_width(480.0)
            .show(ctx, |ui| {
                action = self.ui(ui);
            });
        self.open = open;
        action
    }

    /// Draws the panel's contents into an existing layout.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> AudioSettingsAction {
        let mut action = AudioSettingsAction::None;
        if ui.button("Rescan devices").clicked() {
            action = AudioSettingsAction::Rescan;
        }
        ui.separator();
        ui.label("Output device");
        if ui
            .selectable_label(self.selected.is_none(), "System default")
            .clicked()
            && self.selected.is_some()
        {
            action = AudioSettingsAction::SelectDevice(None);
        }
        match self.devices.as_ref() {
            None => {
                ui.label("scanning…");
            }
            Some(Err(error)) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("audio devices unavailable: {error}"),
                );
            }
            Some(Ok(devices)) if devices.is_empty() => {
                ui.label("this system offers no audio output device");
            }
            Some(Ok(devices)) => {
                for device in devices {
                    let chosen = self.selected.as_deref() == Some(device.id());
                    if ui.selectable_label(chosen, device_label(device)).clicked() && !chosen {
                        action = AudioSettingsAction::SelectDevice(Some(device.id().to_owned()));
                    }
                }
            }
        }
        if let Some(error) = &self.last_error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("could not switch device: {error}"),
            );
        }
        ui.separator();
        ui.label("Scrubbing");
        let mut scrub = self.scrub;
        let mut changed = ui
            .checkbox(&mut scrub.enabled, "Play audio while scrubbing")
            .changed();
        let mut grain_ms = scrub.grain_ms();
        changed |= ui
            .add_enabled(
                scrub.enabled,
                egui::Slider::new(&mut grain_ms, MIN_GRAIN_MS..=MAX_GRAIN_MS).text("grain (ms)"),
            )
            .changed();
        if changed {
            action = AudioSettingsAction::SetScrub(scrub.with_grain_ms(grain_ms));
        }
        ui.separator();
        ui.label(status_line(self.diagnostics.as_ref()));
        if let Some(diagnostics) = &self.diagnostics
            && !diagnostics.is_healthy()
        {
            ui.colored_label(ui.visuals().warn_fg_color, underrun_line(diagnostics));
        }
        action
    }
}

/// One device's line in the list: its name, whether the host would pick it,
/// and what it can do.
pub fn device_label(device: &OutputDeviceInfo) -> String {
    let default = if device.is_default() {
        " (system default)"
    } else {
        ""
    };
    let rate = device
        .default_sample_rate()
        .map_or_else(String::new, |rate| format!(" — {rate} Hz"));
    let channels = device
        .formats()
        .iter()
        .map(sub_audio::SupportedFormat::channels)
        .max()
        .unwrap_or(0);
    let channels = if channels > 0 {
        format!(", up to {channels} ch")
    } else {
        String::new()
    };
    format!("{}{default}{rate}{channels}", device.name())
}

/// The one-line summary of the stream, whether or not one is open.
pub fn status_line(diagnostics: Option<&OutputDiagnostics>) -> String {
    match diagnostics {
        None => "Output closed — nothing is playing".to_owned(),
        Some(diagnostics) => format!(
            "Playing on {}: {}",
            diagnostics.device_name, diagnostics.format
        ),
    }
}

/// What went wrong on a stream that is not healthy.
pub fn underrun_line(diagnostics: &OutputDiagnostics) -> String {
    let last = diagnostics
        .last_error
        .as_ref()
        .map_or_else(String::new, |error| format!(" — last: {error}"));
    format!(
        "{} underrun frames, {} stream errors{last}",
        diagnostics.underrun_frames, diagnostics.stream_errors
    )
}

#[cfg(test)]
mod tests {
    use sub_audio::{NegotiatedFormat, OutputSampleFormat, SupportedFormat, negotiate};

    use super::*;

    fn devices() -> Vec<OutputDeviceInfo> {
        let formats = vec![
            SupportedFormat::new(2, 44_100, 48_000, OutputSampleFormat::F32),
            SupportedFormat::new(6, 48_000, 48_000, OutputSampleFormat::I16),
        ];
        vec![
            OutputDeviceInfo::new("built-in", "Built-in Output", formats.clone())
                .with_default(true)
                .with_default_sample_rate(Some(48_000)),
            OutputDeviceInfo::new("usb", "USB Interface", formats),
        ]
    }

    fn format() -> NegotiatedFormat {
        negotiate(
            &[SupportedFormat::new(
                2,
                44_100,
                44_100,
                OutputSampleFormat::F32,
            )],
            48_000,
            2,
        )
        .expect("a format")
    }

    fn diagnostics() -> OutputDiagnostics {
        OutputDiagnostics {
            device_name: "Built-in Output".to_owned(),
            device_id: "built-in".to_owned(),
            format: format(),
            underrun_frames: 0,
            frames_rendered: 48_000,
            callbacks: 94,
            stream_errors: 0,
            last_error: None,
        }
    }

    #[test]
    fn a_closed_panel_never_asks_for_a_scan() {
        let mut panel = AudioSettingsPanel::new();
        assert!(!panel.needs_devices());
        panel.open = true;
        assert!(panel.needs_devices());
        panel.set_devices(Ok(devices()));
        assert!(!panel.needs_devices());
        panel.refresh();
        assert!(panel.needs_devices());
    }

    #[test]
    fn a_failed_scan_is_remembered_rather_than_repeated() {
        let mut panel = AudioSettingsPanel::new();
        panel.open = true;
        panel.set_devices(Err(SubError::new(
            sub_audio::codes::DEVICE_UNAVAILABLE,
            "the host is gone",
        )));
        assert!(!panel.needs_devices());
        let error = panel
            .devices()
            .expect("a result")
            .expect_err("the scan failed");
        assert_eq!(error.code, sub_audio::codes::DEVICE_UNAVAILABLE);
    }

    #[test]
    fn the_selection_follows_what_the_output_actually_did() {
        let mut panel = AudioSettingsPanel::new();
        assert_eq!(panel.selected(), None);
        panel.set_selected(Some("usb"));
        assert_eq!(panel.selected(), Some("usb"));
        panel.set_error(Some(SubError::new(
            sub_audio::codes::DEVICE_UNAVAILABLE,
            "gone",
        )));
        // The application restored the previous device, so the panel does.
        panel.set_selected(None);
        assert_eq!(panel.selected(), None);
        assert!(panel.last_error().is_some());
        panel.set_error(None);
        assert!(panel.last_error().is_none());
    }

    #[test]
    fn a_device_label_names_the_default_and_what_it_can_do() {
        let devices = devices();
        let label = device_label(&devices[0]);
        assert!(label.contains("Built-in Output"), "{label}");
        assert!(label.contains("system default"), "{label}");
        assert!(label.contains("48000 Hz"), "{label}");
        assert!(label.contains("up to 6 ch"), "{label}");
        assert!(
            !device_label(&devices[1]).contains("system default"),
            "only one device is the default"
        );
    }

    #[test]
    fn the_status_line_says_when_nothing_is_playing() {
        assert!(status_line(None).contains("closed"));
        let line = status_line(Some(&diagnostics()));
        assert!(line.contains("Built-in Output"), "{line}");
        assert!(line.contains("44100 Hz"), "{line}");
        assert!(line.contains("resampled from 48000 Hz"), "{line}");
    }

    #[test]
    fn underruns_and_stream_errors_are_surfaced() {
        let mut diagnostics = diagnostics();
        assert!(diagnostics.is_healthy());
        diagnostics.underrun_frames = 512;
        diagnostics.stream_errors = 2;
        diagnostics.last_error = Some("device disconnected".to_owned());
        assert!(!diagnostics.is_healthy());
        let line = underrun_line(&diagnostics);
        assert!(line.contains("512 underrun frames"), "{line}");
        assert!(line.contains("2 stream errors"), "{line}");
        assert!(line.contains("device disconnected"), "{line}");
    }
}
