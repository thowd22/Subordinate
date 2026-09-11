//! The hardware diagnostics panel: what decoders and encoders this
//! installation registered, and what to install when one is missing.
//!
//! A missing GStreamer plugin is the likeliest support issue (docs/PLAN.md
//! §9), so the panel is reachable from the editor and shows the same report
//! `subordinate-cli diag` prints. The scan touches the GStreamer registry
//! only, which is cheap, but it still runs once on demand and is cached
//! afterwards so the UI thread never repeats it while painting.

use eframe::egui;
use sub_export::{CODECS, EncoderPreferences, EncoderProbe, EncoderStatus, VideoCodec};
use sub_media::{HardwareDiagnostics, VendorReport};

/// The panel's state: the report once it has been collected, and whether the
/// window is open.
#[derive(Default)]
pub struct DiagnosticsPanel {
    /// `None` until the first scan; then the report or the error it failed
    /// with, kept so a failure is shown rather than retried every frame.
    report: Option<Result<HardwareDiagnostics, sub_core::SubError>>,
    /// The user's encoder overrides, so the panel shows what an export would
    /// actually pick rather than what it would pick with default settings.
    pub encoder_preferences: EncoderPreferences,
    /// Whether the window is showing.
    pub open: bool,
}

impl DiagnosticsPanel {
    /// A closed panel that has not scanned anything yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached report, scanning the registry on first use.
    ///
    /// # Errors
    ///
    /// The `media.init_failed` error from [`HardwareDiagnostics::collect`],
    /// remembered so the scan is attempted only once.
    pub fn report(&mut self) -> Result<&HardwareDiagnostics, &sub_core::SubError> {
        self.report
            .get_or_insert_with(HardwareDiagnostics::collect)
            .as_ref()
    }

    /// Forgets the cached report so the next use scans again.
    pub fn refresh(&mut self) {
        self.report = None;
    }

    /// Draws the panel as a window, if it is open.
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Hardware diagnostics")
            .open(&mut open)
            .resizable(true)
            .default_width(560.0)
            .show(ctx, |ui| self.ui(ui));
        self.open = open;
    }

    /// Draws the panel's contents into an existing layout.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if ui.button("Rescan").clicked() {
            self.refresh();
        }
        // Split the borrow by hand: the report is behind `&mut self` because
        // the first use scans, while the preferences the encoder section reads
        // are a plain field.
        let Self {
            report,
            encoder_preferences: preferences,
            ..
        } = self;
        match report
            .get_or_insert_with(HardwareDiagnostics::collect)
            .as_ref()
        {
            Ok(report) => {
                ui.label(summary_line(report));
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for vendor in &report.vendors {
                        vendor_ui(ui, vendor);
                    }
                    ui.separator();
                    encoders_ui(ui, preferences);
                });
            }
            Err(err) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("diagnostics unavailable: {err}"),
                );
            }
        }
    }
}

/// Draws one vendor group: its heading, its elements, and any install hint.
fn vendor_ui(ui: &mut egui::Ui, vendor: &VendorReport) {
    egui::CollapsingHeader::new(vendor_heading(vendor))
        .default_open(vendor.expected)
        .show(ui, |ui| {
            for element in &vendor.elements {
                ui.label(element_line(element));
            }
            if let Some(hint) = &vendor.hint {
                ui.colored_label(ui.visuals().warn_fg_color, hint);
            }
        });
}

/// The one-line header of the whole panel.
fn summary_line(report: &HardwareDiagnostics) -> String {
    let available = report
        .vendors
        .iter()
        .filter(|vendor| vendor.is_available())
        .count();
    format!(
        "GStreamer {} on {}: {available} of {} families available{}",
        report.gstreamer_version,
        report.platform,
        report.vendors.len(),
        if report.has_software_encoder() {
            String::new()
        } else {
            "; no software encoder, so export is unavailable".to_owned()
        }
    )
}

/// Draws the export encoder probe: what each codec would actually encode
/// with, and why the alternatives were passed over.
///
/// The probe is cached for the session by `sub_export`, so painting this every
/// frame costs a lookup and no element instantiation.
fn encoders_ui(ui: &mut egui::Ui, preferences: &EncoderPreferences) {
    ui.label("Export encoders");
    match EncoderProbe::cached() {
        Ok(probe) => {
            for codec in CODECS {
                egui::CollapsingHeader::new(codec_heading(probe, codec, preferences))
                    .id_salt(codec.as_str())
                    .default_open(probe.can_encode(codec))
                    .show(ui, |ui| {
                        for status in probe.for_codec(codec) {
                            ui.label(encoder_line(status));
                        }
                    });
            }
        }
        Err(err) => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("encoder probe unavailable: {err}"),
            );
        }
    }
}

/// One codec's heading: which encoder an export would use, or why none.
fn codec_heading(
    probe: &EncoderProbe,
    codec: VideoCodec,
    preferences: &EncoderPreferences,
) -> String {
    match probe.select(codec, preferences) {
        Ok(status) => {
            let how = if preferences.override_for(codec).is_some() {
                "set in settings"
            } else if status.hardware {
                "hardware, chosen automatically"
            } else {
                "software, chosen automatically"
            };
            format!("{}: {} ({how})", codec.label(), status.element)
        }
        Err(err) => format!("{}: unavailable - {}", codec.label(), err.message),
    }
}

/// One encoder's line: whether it can be used, and what stopped it.
fn encoder_line(status: &EncoderStatus) -> String {
    if status.is_usable() {
        format!("[x] {} ({}) - ready", status.element, status.vendor.label())
    } else if status.deranked && status.ready {
        format!(
            "[ ] {} ({}) - ready, ranked NONE here: used only when it is pinned",
            status.element,
            status.vendor.label()
        )
    } else if status.present {
        format!(
            "[ ] {} ({}) - registered but not usable: {}",
            status.element,
            status.vendor.label(),
            status.detail.as_deref().unwrap_or("no reason given")
        )
    } else {
        format!(
            "[ ] {} ({}) - not registered",
            status.element,
            status.vendor.label()
        )
    }
}

/// One vendor's heading: name, plugin version and how much of it is there.
fn vendor_heading(vendor: &VendorReport) -> String {
    let (decoders, encoders) = vendor.counts();
    let version = vendor
        .plugin_version
        .as_deref()
        .map_or_else(String::new, |version| format!(" {version}"));
    let state = if vendor.is_available() {
        format!("{decoders} decoders, {encoders} encoders")
    } else if vendor.expected {
        "missing".to_owned()
    } else {
        "not available on this platform".to_owned()
    };
    format!("{}{version} - {state}", vendor.vendor.label())
}

/// One element's line: whether it is there, and what backs it.
fn element_line(element: &sub_media::ElementStatus) -> String {
    if element.present {
        let plugin = element.plugin.as_deref().unwrap_or("unknown plugin");
        let version = element
            .plugin_version
            .as_deref()
            .map_or_else(String::new, |version| format!(" {version}"));
        format!(
            "[x] {} ({}) - {plugin}{version}",
            element.name, element.kind
        )
    } else {
        format!("[ ] {} ({}) - not registered", element.name, element.kind)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DiagnosticsPanel, codec_heading, element_line, encoder_line, summary_line, vendor_heading,
    };
    use eframe::egui;
    use sub_export::{CODECS, EncoderPreferences, EncoderProbe, EncoderStatus, VideoCodec};
    use sub_media::{HardwareDiagnostics, Vendor};

    fn scan() -> HardwareDiagnostics {
        HardwareDiagnostics::collect().expect("GStreamer must initialise")
    }

    /// Collects the text of every glyph run in a shape tree.
    fn collect_text(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, into);
                }
            }
            _ => {}
        }
    }

    /// Paints the panel into a headless egui context and returns every string
    /// it actually drew. Two frames, because the first lays the fonts out.
    fn painted_text(panel: &mut DiagnosticsPanel) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for _ in 0..2 {
            texts.clear();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| panel.ui(ui));
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut texts);
            }
            // No painter here consumes the font atlas, so release it by hand
            // rather than let epaint panic on the unapplied delta.
            output.textures_delta.clear();
        }
        texts
    }

    #[test]
    fn the_painted_panel_lists_every_vendor_and_the_elements_of_the_open_ones() {
        let mut panel = DiagnosticsPanel::new();
        let report = scan();
        let painted = painted_text(&mut panel);
        assert!(
            painted.iter().any(|text| text.contains("Rescan")),
            "painted: {painted:?}"
        );
        assert!(
            painted
                .iter()
                .any(|text| text.contains(&report.gstreamer_version)),
            "the summary line is painted: {painted:?}"
        );
        for vendor in &report.vendors {
            assert!(
                painted
                    .iter()
                    .any(|text| text.contains(vendor.vendor.label())),
                "{} is not painted: {painted:?}",
                vendor.vendor
            );
            if !vendor.expected {
                continue;
            }
            for element in &vendor.elements {
                assert!(
                    painted.iter().any(|text| text.contains(&element.name)),
                    "{} is not painted: {painted:?}",
                    element.name
                );
            }
            if let Some(hint) = &vendor.hint {
                assert!(
                    painted.iter().any(|text| text == hint),
                    "the install hint for {} is not painted: {painted:?}",
                    vendor.vendor
                );
            }
        }
    }

    #[test]
    fn a_new_panel_is_closed_and_has_not_scanned() {
        let mut panel = DiagnosticsPanel::new();
        assert!(!panel.open);
        assert!(panel.report().is_ok(), "the first use scans");
    }

    #[test]
    fn the_summary_names_the_version_and_platform() {
        let report = scan();
        let line = summary_line(&report);
        assert!(line.contains(&report.gstreamer_version), "line: {line}");
        assert!(line.contains(&report.platform), "line: {line}");
        assert!(line.contains("families available"), "line: {line}");
    }

    #[test]
    fn every_vendor_gets_a_heading_and_every_element_a_line() {
        let report = scan();
        for vendor in &report.vendors {
            let heading = vendor_heading(vendor);
            assert!(
                heading.starts_with(vendor.vendor.label()),
                "heading: {heading}"
            );
            for element in &vendor.elements {
                let line = element_line(element);
                assert!(line.contains(&element.name), "line: {line}");
                assert!(line.contains(element.kind.as_str()), "line: {line}");
                assert_eq!(line.starts_with("[x]"), element.present, "line: {line}");
            }
        }
    }

    #[test]
    fn the_painted_panel_shows_the_encoder_probe_for_every_codec() {
        let mut panel = DiagnosticsPanel::new();
        let probe = EncoderProbe::cached().expect("GStreamer must initialise");
        let painted = painted_text(&mut panel);
        assert!(
            painted.iter().any(|text| text == "Export encoders"),
            "painted: {painted:?}"
        );
        for codec in CODECS {
            assert!(
                painted.iter().any(|text| text.starts_with(codec.label())),
                "{codec} has no heading: {painted:?}"
            );
            if !probe.can_encode(codec) {
                continue;
            }
            for status in probe.for_codec(codec) {
                assert!(
                    painted.iter().any(|text| text.contains(&status.element)),
                    "{} is not painted: {painted:?}",
                    status.element
                );
            }
        }
    }

    #[test]
    fn the_codec_heading_names_the_encoder_an_export_would_use() {
        let probe = EncoderProbe::cached().expect("GStreamer must initialise");
        for codec in CODECS {
            let heading = codec_heading(probe, codec, &EncoderPreferences::new());
            assert!(heading.starts_with(codec.label()), "heading: {heading}");
            match probe.select(codec, &EncoderPreferences::new()) {
                Ok(status) => {
                    assert!(heading.contains(&status.element), "heading: {heading}");
                    assert!(
                        heading.contains("chosen automatically"),
                        "heading: {heading}"
                    );
                }
                Err(_) => assert!(heading.contains("unavailable"), "heading: {heading}"),
            }
        }
    }

    #[test]
    fn an_override_is_labelled_as_coming_from_settings() {
        let probe = EncoderProbe::cached().expect("GStreamer must initialise");
        let mut preferences = EncoderPreferences::new();
        preferences
            .set_override(VideoCodec::H264, "x264enc")
            .expect("x264enc encodes H.264");
        let heading = codec_heading(probe, VideoCodec::H264, &preferences);
        assert!(heading.contains("x264enc"), "heading: {heading}");
        if probe
            .status("x264enc")
            .is_some_and(sub_export::EncoderStatus::is_usable)
        {
            assert!(heading.contains("set in settings"), "heading: {heading}");
        } else {
            assert!(heading.contains("unavailable"), "heading: {heading}");
        }
    }

    #[test]
    fn every_encoder_gets_a_line_that_says_whether_it_is_usable() {
        let probe = EncoderProbe::cached().expect("GStreamer must initialise");
        for status in &probe.encoders {
            let line = encoder_line(status);
            assert!(line.contains(&status.element), "line: {line}");
            assert_eq!(line.starts_with("[x]"), status.is_usable(), "line: {line}");
        }
    }

    /// A hardware encoder ranked NONE runs here but is never picked by the
    /// automatic order, so its line has to say both (TASK-134).
    #[test]
    fn a_deranked_encoder_reads_as_pinned_only() {
        let status = EncoderStatus {
            element: "vah264enc".to_owned(),
            codec: VideoCodec::H264,
            vendor: sub_export::EncoderVendor::Va,
            hardware: true,
            present: true,
            ready: true,
            deranked: true,
            detail: Some("the element is ranked NONE on this machine".to_owned()),
        };
        let line = encoder_line(&status);
        assert!(line.starts_with("[ ]"), "line: {line}");
        assert!(line.contains("ranked NONE"), "line: {line}");
        assert!(line.contains("pinned"), "line: {line}");
    }

    #[test]
    fn an_unavailable_family_says_so_in_its_heading() {
        let report = scan();
        let vendor = report.vendor(Vendor::Amf).expect("amf is reported");
        if !vendor.is_available() {
            let heading = vendor_heading(vendor);
            assert!(
                heading.contains("missing") || heading.contains("not available"),
                "heading: {heading}"
            );
        }
    }
}
