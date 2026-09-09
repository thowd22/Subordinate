//! Hardware diagnostics: which decoder and encoder elements this GStreamer
//! installation actually registered (docs/PLAN.md §9).
//!
//! A missing plugin is the most likely support issue, and neither a user nor
//! an agent can see one without asking the registry. [`HardwareDiagnostics`]
//! walks a fixed catalogue of the elements the editor cares about, records the
//! plugin and version behind each one, and attaches the install step from
//! `docs/DEVELOPMENT.md` to every vendor that should be present on this
//! platform but is not.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! let diagnostics = sub_media::HardwareDiagnostics::collect()?;
//! for hint in diagnostics.hints() {
//!     println!("{hint}");
//! }
//! # Ok(())
//! # }
//! ```

use gstreamer as gst;
use gstreamer::glib::translate::IntoGlib;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};

use crate::codes;

/// Whether an element decodes or encodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    /// Turns a coded stream into frames.
    Decoder,
    /// Turns frames into a coded stream.
    Encoder,
}

impl ElementKind {
    /// The stable identifier used in JSON and in the panel.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decoder => "decoder",
            Self::Encoder => "encoder",
        }
    }
}

impl std::fmt::Display for ElementKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A family of elements from one hardware or software backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    /// NVIDIA NVDEC and NVENC, from the `nvcodec` plugin.
    Nvcodec,
    /// VA-API on Linux (AMD and Intel), from the `va` plugin.
    Va,
    /// AMD Advanced Media Framework on Windows, from the `amfcodec` plugin.
    Amf,
    /// Apple VideoToolbox on macOS, from the `applemedia` plugin.
    Vtenc,
    /// Windows Media Foundation, from the `mediafoundation` plugin.
    Mf,
    /// Software fallback: `x264`, `x265` and the libav decoders.
    X264,
}

/// Every vendor, in the order they are reported.
pub const VENDORS: [Vendor; 6] = [
    Vendor::Nvcodec,
    Vendor::Va,
    Vendor::Amf,
    Vendor::Vtenc,
    Vendor::Mf,
    Vendor::X264,
];

impl Vendor {
    /// The stable identifier used in JSON and in the panel.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nvcodec => "nvcodec",
            Self::Va => "va",
            Self::Amf => "amf",
            Self::Vtenc => "vtenc",
            Self::Mf => "mf",
            Self::X264 => "x264",
        }
    }

    /// A human-readable name for the panel.
    pub fn label(self) -> &'static str {
        match self {
            Self::Nvcodec => "NVIDIA (nvcodec)",
            Self::Va => "AMD/Intel VA-API (va)",
            Self::Amf => "AMD AMF (amfcodec)",
            Self::Vtenc => "Apple VideoToolbox (applemedia)",
            Self::Mf => "Windows Media Foundation",
            Self::X264 => "Software (x264/x265/libav)",
        }
    }

    /// The GStreamer plugin the elements come from, when they all come from
    /// one. The software family spans several plugins and reports `None`.
    pub fn plugin(self) -> Option<&'static str> {
        match self {
            Self::Nvcodec => Some("nvcodec"),
            Self::Va => Some("va"),
            Self::Amf => Some("amfcodec"),
            Self::Vtenc => Some("applemedia"),
            Self::Mf => Some("mediafoundation"),
            Self::X264 => None,
        }
    }

    /// Whether this vendor is expected to be present on `os`, which is a
    /// [`std::env::consts::OS`] value.
    ///
    /// Only an expected vendor that is missing elements earns a hint: nobody
    /// needs to be told that AMF is absent on Linux.
    pub fn is_expected_on(self, os: &str) -> bool {
        match self {
            Self::Nvcodec => matches!(os, "linux" | "windows"),
            Self::Va => os == "linux",
            Self::Amf | Self::Mf => os == "windows",
            Self::Vtenc => os == "macos",
            Self::X264 => true,
        }
    }

    /// The install step from `docs/DEVELOPMENT.md` for this vendor on `os`.
    ///
    /// `None` when the vendor is not expected there at all.
    pub fn install_hint(self, os: &str) -> Option<&'static str> {
        if !self.is_expected_on(os) {
            return None;
        }
        Some(match (self, os) {
            (Self::Nvcodec, "linux") => {
                "nvcodec ships in gstreamer1.0-plugins-bad and needs the NVIDIA driver at \
                 runtime: sudo apt-get install -y gstreamer1.0-plugins-bad \
                 (docs/DEVELOPMENT.md, GStreamer > Linux)."
            }
            (Self::Va, _) => {
                "The va plugin is not packaged by Ubuntu and gstreamer-vaapi was removed \
                 upstream in 1.28: use Fedora 44+, Arch, or Homebrew on Linux \
                 (brew install gstreamer, then export \
                 PKG_CONFIG_PATH=$(brew --prefix)/lib/pkgconfig) \
                 (docs/DEVELOPMENT.md, GStreamer > Linux)."
            }
            (Self::Vtenc, _) => {
                "Install both the runtime and devel packages from \
                 https://gstreamer.freedesktop.org/data/pkg/osx/<version>/; the macOS build \
                 includes applemedia (docs/DEVELOPMENT.md, GStreamer > macOS)."
            }
            (Self::X264, "linux") => {
                "x264enc is in gstreamer1.0-plugins-ugly and the libav decoders in \
                 gstreamer1.0-libav: sudo apt-get install -y gstreamer1.0-plugins-ugly \
                 gstreamer1.0-libav (docs/DEVELOPMENT.md, GStreamer > Linux)."
            }
            _ => {
                "Install the official GStreamer MSVC x86_64 build for the pinned version \
                 from https://gstreamer.freedesktop.org/data/pkg/windows/<version>/msvc/ and \
                 put its bin directory on PATH; that build includes nvcodec, amfcodec and \
                 mediafoundation (docs/DEVELOPMENT.md, GStreamer > Windows)."
            }
        })
    }
}

impl std::fmt::Display for Vendor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One element the editor looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Expected {
    name: &'static str,
    kind: ElementKind,
    vendor: Vendor,
}

const fn decoder(name: &'static str, vendor: Vendor) -> Expected {
    Expected {
        name,
        kind: ElementKind::Decoder,
        vendor,
    }
}

const fn encoder(name: &'static str, vendor: Vendor) -> Expected {
    Expected {
        name,
        kind: ElementKind::Encoder,
        vendor,
    }
}

/// The catalogue: every decoder and encoder the editor can use, following the
/// selection order in docs/PLAN.md §5.4.
const CATALOGUE: &[Expected] = &[
    decoder("nvh264dec", Vendor::Nvcodec),
    decoder("nvh265dec", Vendor::Nvcodec),
    decoder("nvav1dec", Vendor::Nvcodec),
    encoder("nvh264enc", Vendor::Nvcodec),
    encoder("nvh265enc", Vendor::Nvcodec),
    encoder("nvav1enc", Vendor::Nvcodec),
    decoder("vah264dec", Vendor::Va),
    decoder("vah265dec", Vendor::Va),
    decoder("vaav1dec", Vendor::Va),
    encoder("vah264enc", Vendor::Va),
    encoder("vah265enc", Vendor::Va),
    encoder("amfh264enc", Vendor::Amf),
    encoder("amfh265enc", Vendor::Amf),
    encoder("amfav1enc", Vendor::Amf),
    decoder("vtdec_hw", Vendor::Vtenc),
    encoder("vtenc_h264", Vendor::Vtenc),
    encoder("vtenc_h265", Vendor::Vtenc),
    decoder("mfh264dec", Vendor::Mf),
    encoder("mfh264enc", Vendor::Mf),
    encoder("mfh265enc", Vendor::Mf),
    decoder("avdec_h264", Vendor::X264),
    decoder("avdec_h265", Vendor::X264),
    encoder("x264enc", Vendor::X264),
    encoder("x265enc", Vendor::X264),
];

/// What the registry says about one element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementStatus {
    /// The GStreamer element factory name, for example `nvh264enc`.
    pub name: String,
    /// Whether it decodes or encodes.
    pub kind: ElementKind,
    /// Whether the factory is registered on this machine.
    pub present: bool,
    /// The plugin the factory came from, when it is present.
    pub plugin: Option<String>,
    /// That plugin's version, when it is present.
    pub plugin_version: Option<String>,
    /// The factory's rank, which is how GStreamer orders autoplugging.
    pub rank: Option<u32>,
}

/// Every element of one vendor, and what to do when they are missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorReport {
    /// The family these elements belong to.
    pub vendor: Vendor,
    /// The plugin behind the family, when it has exactly one.
    pub plugin: Option<String>,
    /// The version of that plugin, taken from the first present element.
    pub plugin_version: Option<String>,
    /// Whether the family is expected on this platform at all.
    pub expected: bool,
    /// Every catalogued element of this family, present or not.
    pub elements: Vec<ElementStatus>,
    /// The install step to show when the family is expected and incomplete.
    pub hint: Option<String>,
}

impl VendorReport {
    /// Element names that are expected here but were not registered.
    pub fn missing(&self) -> Vec<&str> {
        self.elements
            .iter()
            .filter(|element| !element.present)
            .map(|element| element.name.as_str())
            .collect()
    }

    /// True when at least one element of the family is registered.
    pub fn is_available(&self) -> bool {
        self.elements.iter().any(|element| element.present)
    }

    /// Present decoders and encoders, counted separately.
    pub fn counts(&self) -> (usize, usize) {
        let decoders = self
            .elements
            .iter()
            .filter(|element| element.present && element.kind == ElementKind::Decoder)
            .count();
        let encoders = self
            .elements
            .iter()
            .filter(|element| element.present && element.kind == ElementKind::Encoder)
            .count();
        (decoders, encoders)
    }
}

/// The whole picture: GStreamer's version and every vendor's elements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareDiagnostics {
    /// The GStreamer runtime version, for example `1.28.6`.
    pub gstreamer_version: String,
    /// The platform the scan ran on, a [`std::env::consts::OS`] value.
    pub platform: String,
    /// One report per vendor, in [`VENDORS`] order.
    pub vendors: Vec<VendorReport>,
}

impl HardwareDiagnostics {
    /// Scans the GStreamer registry of this machine.
    ///
    /// # Errors
    ///
    /// `media.init_failed` when GStreamer itself cannot be initialised, which
    /// is the only way the scan can fail: a missing element is a result, not
    /// an error.
    pub fn collect() -> SubResult<Self> {
        gst::init()
            .map_err(|e| SubError::wrap(codes::INIT_FAILED, "GStreamer failed to start", &e))?;
        let (major, minor, micro, _) = gst::version();
        Ok(Self::from_registry(
            &format!("{major}.{minor}.{micro}"),
            std::env::consts::OS,
            &registry_lookup,
        ))
    }

    /// Builds a report from an arbitrary element lookup, which is what lets
    /// the tests describe a machine without having one.
    fn from_registry(
        gstreamer_version: &str,
        os: &str,
        lookup: &dyn Fn(&str) -> Option<FoundElement>,
    ) -> Self {
        let vendors = VENDORS
            .iter()
            .map(|&vendor| vendor_report(vendor, os, lookup))
            .collect();
        Self {
            gstreamer_version: gstreamer_version.to_owned(),
            platform: os.to_owned(),
            vendors,
        }
    }

    /// The report for one vendor.
    pub fn vendor(&self, vendor: Vendor) -> Option<&VendorReport> {
        self.vendors.iter().find(|report| report.vendor == vendor)
    }

    /// Vendors that should be present on this platform but are missing at
    /// least one element.
    pub fn incomplete(&self) -> Vec<&VendorReport> {
        self.vendors
            .iter()
            .filter(|report| report.hint.is_some())
            .collect()
    }

    /// One install hint per incomplete vendor, ready to print or show.
    pub fn hints(&self) -> Vec<String> {
        self.incomplete()
            .iter()
            .filter_map(|report| {
                report.hint.as_ref().map(|hint| {
                    format!(
                        "{}: missing {}. {hint}",
                        report.vendor.label(),
                        report.missing().join(", ")
                    )
                })
            })
            .collect()
    }

    /// True when a software encoder is registered. Without one this
    /// installation cannot export at all.
    pub fn has_software_encoder(&self) -> bool {
        self.vendor(Vendor::X264).is_some_and(|report| {
            report
                .elements
                .iter()
                .any(|element| element.present && element.kind == ElementKind::Encoder)
        })
    }

    /// The report as JSON, which is what `subordinate-cli diag` prints.
    ///
    /// # Errors
    ///
    /// `media.probe_failed` if the report cannot be serialised, which cannot
    /// happen for the shapes above but is reported rather than panicked on.
    pub fn to_json(&self) -> SubResult<serde_json::Value> {
        serde_json::to_value(self).map_err(|e| {
            SubError::wrap(codes::PROBE_FAILED, "diagnostics are not serialisable", &e)
        })
    }
}

/// What a lookup found for one element name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundElement {
    /// The plugin the factory belongs to.
    pub plugin: Option<String>,
    /// That plugin's version.
    pub plugin_version: Option<String>,
    /// The factory's autoplug rank.
    pub rank: u32,
}

/// Looks one element up in the real GStreamer registry.
fn registry_lookup(name: &str) -> Option<FoundElement> {
    let factory = gst::ElementFactory::find(name)?;
    let plugin = factory.plugin_name().map(|plugin| plugin.to_string());
    let plugin_version = plugin
        .as_deref()
        .and_then(|plugin| gst::Registry::get().find_plugin(plugin))
        .map(|plugin| plugin.version().to_string());
    Some(FoundElement {
        plugin,
        plugin_version,
        rank: u32::try_from(factory.rank().into_glib()).unwrap_or(0),
    })
}

/// Builds one vendor's report from the catalogue and a lookup.
fn vendor_report(
    vendor: Vendor,
    os: &str,
    lookup: &dyn Fn(&str) -> Option<FoundElement>,
) -> VendorReport {
    let elements: Vec<ElementStatus> = CATALOGUE
        .iter()
        .filter(|expected| expected.vendor == vendor)
        .map(|expected| {
            let found = lookup(expected.name);
            ElementStatus {
                name: expected.name.to_owned(),
                kind: expected.kind,
                present: found.is_some(),
                plugin: found.as_ref().and_then(|found| found.plugin.clone()),
                plugin_version: found
                    .as_ref()
                    .and_then(|found| found.plugin_version.clone()),
                rank: found.as_ref().map(|found| found.rank),
            }
        })
        .collect();

    let expected = vendor.is_expected_on(os);
    let any_missing = elements.iter().any(|element| !element.present);
    let hint = if expected && any_missing {
        vendor.install_hint(os).map(str::to_owned)
    } else {
        None
    };
    let plugin_version = elements
        .iter()
        .find_map(|element| element.plugin_version.clone());

    VendorReport {
        vendor,
        plugin: vendor.plugin().map(str::to_owned),
        plugin_version,
        expected,
        elements,
        hint,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CATALOGUE, ElementKind, FoundElement, HardwareDiagnostics, VENDORS, Vendor, vendor_report,
    };

    fn nothing_found(_: &str) -> Option<FoundElement> {
        None
    }

    fn everything_found(name: &str) -> Option<FoundElement> {
        (!name.is_empty()).then(|| FoundElement {
            plugin: Some("nvcodec".to_owned()),
            plugin_version: Some("1.28.6".to_owned()),
            rank: 256,
        })
    }

    #[test]
    fn the_catalogue_covers_every_vendor_with_unique_names() {
        for vendor in VENDORS {
            assert!(
                CATALOGUE.iter().any(|e| e.vendor == vendor),
                "{vendor} has no catalogued elements"
            );
        }
        let mut names: Vec<&str> = CATALOGUE.iter().map(|e| e.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "element names must be unique");
    }

    #[test]
    fn every_family_lists_an_encoder_and_amf_is_encode_only() {
        for vendor in VENDORS {
            let encoders = CATALOGUE
                .iter()
                .filter(|e| e.vendor == vendor && e.kind == ElementKind::Encoder)
                .count();
            assert!(encoders > 0, "{vendor} lists no encoder");
        }
        let amf_decoders = CATALOGUE
            .iter()
            .filter(|e| e.vendor == Vendor::Amf && e.kind == ElementKind::Decoder)
            .count();
        assert_eq!(amf_decoders, 0, "amfcodec is encode-only for us");
    }

    #[test]
    fn vendors_are_expected_only_on_the_platforms_that_ship_them() {
        assert!(Vendor::Va.is_expected_on("linux"));
        assert!(!Vendor::Va.is_expected_on("windows"));
        assert!(Vendor::Amf.is_expected_on("windows"));
        assert!(!Vendor::Amf.is_expected_on("macos"));
        assert!(Vendor::Vtenc.is_expected_on("macos"));
        assert!(!Vendor::Vtenc.is_expected_on("linux"));
        assert!(Vendor::Nvcodec.is_expected_on("linux"));
        assert!(!Vendor::Nvcodec.is_expected_on("macos"));
        for os in ["linux", "windows", "macos"] {
            assert!(Vendor::X264.is_expected_on(os), "software is always wanted");
        }
    }

    #[test]
    fn hints_name_the_development_guide_and_only_apply_where_expected() {
        for vendor in VENDORS {
            for os in ["linux", "windows", "macos"] {
                match vendor.install_hint(os) {
                    Some(hint) => {
                        assert!(vendor.is_expected_on(os));
                        assert!(
                            hint.contains("docs/DEVELOPMENT.md"),
                            "{vendor} on {os} must cite the install guide"
                        );
                    }
                    None => assert!(!vendor.is_expected_on(os)),
                }
            }
        }
    }

    #[test]
    fn a_bare_machine_reports_every_element_missing_with_a_hint_per_expected_vendor() {
        let report = HardwareDiagnostics::from_registry("1.28.6", "linux", &nothing_found);
        assert_eq!(report.platform, "linux");
        assert_eq!(report.vendors.len(), VENDORS.len());
        assert!(!report.has_software_encoder());
        for vendor in &report.vendors {
            assert!(!vendor.is_available(), "{} should be absent", vendor.vendor);
            assert_eq!(vendor.missing().len(), vendor.elements.len());
            assert_eq!(vendor.hint.is_some(), vendor.expected);
        }
        let hinted: Vec<Vendor> = report.incomplete().iter().map(|v| v.vendor).collect();
        assert_eq!(hinted, vec![Vendor::Nvcodec, Vendor::Va, Vendor::X264]);
        assert_eq!(report.hints().len(), 3);
        assert!(report.hints()[0].contains("nvh264enc"));
    }

    #[test]
    fn a_complete_machine_needs_no_hints_and_reports_versions() {
        let report = HardwareDiagnostics::from_registry("1.28.6", "windows", &everything_found);
        assert!(report.incomplete().is_empty());
        assert!(report.hints().is_empty());
        assert!(report.has_software_encoder());
        let nvcodec = report.vendor(Vendor::Nvcodec).expect("nvcodec reported");
        assert_eq!(nvcodec.plugin_version.as_deref(), Some("1.28.6"));
        assert_eq!(nvcodec.counts(), (3, 3));
        assert!(nvcodec.elements.iter().all(|e| e.rank == Some(256)));
    }

    #[test]
    fn a_partial_family_is_incomplete_even_though_some_elements_are_present() {
        let lookup = |name: &str| {
            (name == "x264enc").then(|| FoundElement {
                plugin: Some("x264".to_owned()),
                plugin_version: Some("1.28.6".to_owned()),
                rank: 128,
            })
        };
        let report = vendor_report(Vendor::X264, "linux", &lookup);
        assert!(report.is_available());
        assert_eq!(report.counts(), (0, 1));
        assert_eq!(
            report.missing(),
            vec!["avdec_h264", "avdec_h265", "x265enc"]
        );
        assert!(report.hint.is_some());
        assert_eq!(report.plugin, None, "the software family spans plugins");
        assert_eq!(report.plugin_version.as_deref(), Some("1.28.6"));
    }

    #[test]
    fn the_json_shape_is_stable() {
        let report = HardwareDiagnostics::from_registry("1.28.6", "macos", &nothing_found);
        let json = report.to_json().expect("diagnostics serialise");
        assert_eq!(json["gstreamer_version"], "1.28.6");
        assert_eq!(json["platform"], "macos");
        let vendors = json["vendors"].as_array().expect("vendors are an array");
        assert_eq!(vendors.len(), VENDORS.len());
        assert_eq!(vendors[0]["vendor"], "nvcodec");
        assert_eq!(vendors[0]["expected"], false);
        assert_eq!(vendors[0]["elements"][0]["name"], "nvh264dec");
        assert_eq!(vendors[0]["elements"][0]["kind"], "decoder");
        assert_eq!(vendors[0]["elements"][0]["present"], false);
        let round_tripped: HardwareDiagnostics =
            serde_json::from_value(json).expect("diagnostics round-trip");
        assert_eq!(round_tripped, report);
    }

    #[test]
    fn this_machine_can_be_scanned() {
        let report = HardwareDiagnostics::collect().expect("GStreamer must initialise");
        assert!(report.gstreamer_version.starts_with("1."));
        assert_eq!(report.platform, std::env::consts::OS);
        assert_eq!(report.vendors.len(), VENDORS.len());
        for vendor in &report.vendors {
            for element in &vendor.elements {
                assert_eq!(
                    element.present,
                    element.plugin.is_some(),
                    "{} reports a plugin only when present",
                    element.name
                );
            }
        }
    }
}
