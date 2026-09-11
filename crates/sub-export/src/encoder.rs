//! Encoder capability probe and selection order (docs/PLAN.md §5.5).
//!
//! Which encoder an export should use is a per-machine question: the element
//! may not be registered at all, or it may be registered by a plugin whose
//! driver is missing, in which case the factory builds but the element never
//! reaches `READY`. The probe answers both by actually instantiating every
//! catalogued encoder and asking it to go to `READY`, then selection walks the
//! plan's order per codec and takes the first encoder that got there.
//!
//! The scan costs real element instantiation, so it is cached for the life of
//! the process ([`EncoderProbe::cached`]) and shown in the diagnostics panel
//! next to the registry report from `sub_media::HardwareDiagnostics`.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use sub_export::{EncoderPreferences, EncoderProbe, VideoCodec};
//!
//! let probe = EncoderProbe::cached()?;
//! let prefs = EncoderPreferences::default();
//! let chosen = probe.select(VideoCodec::H264, &prefs)?;
//! println!("encoding with {}", chosen.element);
//! # Ok(())
//! # }
//! ```

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use gstreamer as gst;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};

use crate::codes;

/// A video codec an export can target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    /// H.264 / AVC.
    H264,
    /// H.265 / HEVC.
    H265,
    /// AV1.
    Av1,
}

/// Every codec, in the order they are reported.
pub const CODECS: [VideoCodec; 3] = [VideoCodec::H264, VideoCodec::H265, VideoCodec::Av1];

impl VideoCodec {
    /// The stable identifier used in JSON, settings and the panel.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::H265 => "h265",
            Self::Av1 => "av1",
        }
    }

    /// A human-readable name for the panel.
    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::H265 => "H.265",
            Self::Av1 => "AV1",
        }
    }

    /// Parses the stable identifier, for settings files and agent calls.
    pub fn parse(name: &str) -> Option<Self> {
        CODECS.into_iter().find(|codec| codec.as_str() == name)
    }
}

impl std::fmt::Display for VideoCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The backend an encoder element belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncoderVendor {
    /// NVIDIA NVENC, from the `nvcodec` plugin.
    Nvenc,
    /// VA-API on Linux, which is how AMD and Intel encode there.
    Va,
    /// AMD Advanced Media Framework on Windows.
    Amf,
    /// Apple VideoToolbox on macOS.
    VideoToolbox,
    /// Windows Media Foundation, the Windows fallback.
    MediaFoundation,
    /// Software: `x264enc` and `x265enc`.
    Software,
}

impl EncoderVendor {
    /// The stable identifier used in JSON and in the panel.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nvenc => "nvenc",
            Self::Va => "va",
            Self::Amf => "amf",
            Self::VideoToolbox => "videotoolbox",
            Self::MediaFoundation => "mediafoundation",
            Self::Software => "software",
        }
    }

    /// A human-readable name for the panel.
    pub fn label(self) -> &'static str {
        match self {
            Self::Nvenc => "NVIDIA NVENC",
            Self::Va => "VA-API",
            Self::Amf => "AMD AMF",
            Self::VideoToolbox => "Apple VideoToolbox",
            Self::MediaFoundation => "Windows Media Foundation",
            Self::Software => "Software",
        }
    }

    /// Whether this backend encodes on the GPU.
    pub fn is_hardware(self) -> bool {
        self != Self::Software
    }
}

impl std::fmt::Display for EncoderVendor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One catalogued encoder element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Candidate {
    element: &'static str,
    codec: VideoCodec,
    vendor: EncoderVendor,
}

const fn candidate(element: &'static str, codec: VideoCodec, vendor: EncoderVendor) -> Candidate {
    Candidate {
        element,
        codec,
        vendor,
    }
}

/// Every encoder the exporter can use, in the selection order of
/// docs/PLAN.md §5.5: NVIDIA, then VA-API, then AMF, then VideoToolbox, then
/// Media Foundation, then software. Selection filters this list by codec and
/// keeps the order, so the list is the order.
const CATALOGUE: &[Candidate] = &[
    candidate("nvh264enc", VideoCodec::H264, EncoderVendor::Nvenc),
    candidate("nvh265enc", VideoCodec::H265, EncoderVendor::Nvenc),
    candidate("nvav1enc", VideoCodec::Av1, EncoderVendor::Nvenc),
    candidate("vah264enc", VideoCodec::H264, EncoderVendor::Va),
    candidate("vah265enc", VideoCodec::H265, EncoderVendor::Va),
    candidate("amfh264enc", VideoCodec::H264, EncoderVendor::Amf),
    candidate("amfh265enc", VideoCodec::H265, EncoderVendor::Amf),
    candidate("vtenc_h264", VideoCodec::H264, EncoderVendor::VideoToolbox),
    candidate("vtenc_h265", VideoCodec::H265, EncoderVendor::VideoToolbox),
    candidate(
        "mfh264enc",
        VideoCodec::H264,
        EncoderVendor::MediaFoundation,
    ),
    candidate("x264enc", VideoCodec::H264, EncoderVendor::Software),
    candidate("x265enc", VideoCodec::H265, EncoderVendor::Software),
];

/// What probing one element found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ElementProbe {
    /// Whether the element factory is registered and could be instantiated.
    pub present: bool,
    /// Whether the instantiated element reached `READY`.
    pub ready: bool,
    /// Why it did not, when it did not: the GStreamer failure, rendered.
    pub detail: Option<String>,
}

impl ElementProbe {
    /// The element is not registered on this machine.
    pub fn missing() -> Self {
        Self::default()
    }

    /// The element is registered and reached `READY`.
    pub fn ready() -> Self {
        Self {
            present: true,
            ready: true,
            detail: None,
        }
    }

    /// The element is registered but could not reach `READY`.
    pub fn not_ready(detail: impl Into<String>) -> Self {
        Self {
            present: true,
            ready: false,
            detail: Some(detail.into()),
        }
    }
}

/// What the probe found about one catalogued encoder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncoderStatus {
    /// The GStreamer element factory name, for example `nvh264enc`.
    pub element: String,
    /// The codec it produces.
    pub codec: VideoCodec,
    /// The backend it belongs to.
    pub vendor: EncoderVendor,
    /// Whether the backend encodes on the GPU.
    pub hardware: bool,
    /// Whether the factory is registered and could be instantiated.
    pub present: bool,
    /// Whether the element reached `READY`, which is what makes it usable.
    pub ready: bool,
    /// Why it is unusable, when it is: the rendered GStreamer failure.
    pub detail: Option<String>,
}

impl EncoderStatus {
    /// True when this encoder can actually be used for an export.
    pub fn is_usable(&self) -> bool {
        self.present && self.ready
    }

    /// A one-line description for the diagnostics panel.
    pub fn summary(&self) -> String {
        let state = if self.ready {
            "ready".to_owned()
        } else if self.present {
            format!(
                "not ready ({})",
                self.detail.as_deref().unwrap_or("no reason given")
            )
        } else {
            "missing".to_owned()
        };
        format!(
            "{} ({}, {}): {state}",
            self.element,
            self.codec.label(),
            self.vendor.label()
        )
    }
}

/// A user's encoder overrides, as they are stored in settings.
///
/// An override pins one codec to one catalogued element; every other codec
/// keeps the plan's order. Overrides are validated when they are set, so a
/// settings file can never name an element the exporter does not know.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EncoderPreferences {
    /// Per-codec pinned element name.
    overrides: BTreeMap<VideoCodec, String>,
}

impl EncoderPreferences {
    /// Preferences with no overrides: pure plan order.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pins `element` as the encoder for `codec`.
    ///
    /// # Errors
    ///
    /// `export.unknown_encoder` when `element` is not a catalogued encoder for
    /// `codec`. Whether it is *available* is a probe question, answered by
    /// [`EncoderProbe::select`], not here: settings are portable between
    /// machines, so an override for hardware this machine lacks is still worth
    /// storing.
    pub fn set_override(&mut self, codec: VideoCodec, element: &str) -> SubResult<()> {
        if !CATALOGUE
            .iter()
            .any(|entry| entry.codec == codec && entry.element == element)
        {
            return Err(SubError::new(
                codes::UNKNOWN_ENCODER,
                format!("{element} is not a known {codec} encoder"),
            )
            .with_detail("codec", codec.as_str())
            .with_detail("element", element)
            .with_detail("known", encoder_names(codec)));
        }
        self.overrides.insert(codec, element.to_owned());
        Ok(())
    }

    /// Removes the override for `codec`, returning the element it named.
    pub fn clear_override(&mut self, codec: VideoCodec) -> Option<String> {
        self.overrides.remove(&codec)
    }

    /// The element pinned for `codec`, if any.
    pub fn override_for(&self, codec: VideoCodec) -> Option<&str> {
        self.overrides.get(&codec).map(String::as_str)
    }

    /// Every override, in codec order.
    pub fn overrides(&self) -> impl Iterator<Item = (VideoCodec, &str)> {
        self.overrides
            .iter()
            .map(|(&codec, element)| (codec, element.as_str()))
    }

    /// True when nothing is pinned.
    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
    }
}

/// The catalogued encoder names for `codec`, in selection order.
pub fn encoder_names(codec: VideoCodec) -> Vec<&'static str> {
    CATALOGUE
        .iter()
        .filter(|entry| entry.codec == codec)
        .map(|entry| entry.element)
        .collect()
}

/// The result of the probe: every catalogued encoder and what it can do here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncoderProbe {
    /// The platform the probe ran on, a [`std::env::consts::OS`] value.
    pub platform: String,
    /// Every catalogued encoder, in selection order.
    pub encoders: Vec<EncoderStatus>,
}

/// The per-session cache: the probe instantiates real elements, so it runs
/// once and every later caller reads the same answer.
static CACHED: OnceLock<Result<EncoderProbe, SubError>> = OnceLock::new();

impl EncoderProbe {
    /// Probes this machine, instantiating every catalogued encoder and driving
    /// it to `READY`.
    ///
    /// # Errors
    ///
    /// `export.init_failed` when GStreamer itself cannot be initialised, which
    /// is the only way the probe can fail: an unusable encoder is a result,
    /// not an error.
    pub fn collect() -> SubResult<Self> {
        gst::init()
            .map_err(|e| SubError::wrap(codes::INIT_FAILED, "GStreamer failed to start", &e))?;
        Ok(Self::from_probe(std::env::consts::OS, &probe_element))
    }

    /// The probe for this session, running it on first use.
    ///
    /// The result — including a failure — is remembered for the life of the
    /// process, so the scan never repeats while the UI paints.
    ///
    /// # Errors
    ///
    /// The `export.init_failed` error from [`EncoderProbe::collect`].
    pub fn cached() -> SubResult<&'static Self> {
        CACHED
            .get_or_init(Self::collect)
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Builds a probe result from an arbitrary element probe, which is what
    /// lets the tests describe a machine without having one.
    fn from_probe(os: &str, probe: &dyn Fn(&str) -> ElementProbe) -> Self {
        let encoders = CATALOGUE
            .iter()
            .map(|entry| {
                let found = probe(entry.element);
                EncoderStatus {
                    element: entry.element.to_owned(),
                    codec: entry.codec,
                    vendor: entry.vendor,
                    hardware: entry.vendor.is_hardware(),
                    present: found.present,
                    ready: found.present && found.ready,
                    detail: found.detail,
                }
            })
            .collect();
        Self {
            platform: os.to_owned(),
            encoders,
        }
    }

    /// Every catalogued encoder for `codec`, in selection order.
    pub fn for_codec(&self, codec: VideoCodec) -> impl Iterator<Item = &EncoderStatus> {
        self.encoders
            .iter()
            .filter(move |status| status.codec == codec)
    }

    /// The usable encoders for `codec`, best first.
    pub fn usable(&self, codec: VideoCodec) -> Vec<&EncoderStatus> {
        self.for_codec(codec)
            .filter(|status| status.is_usable())
            .collect()
    }

    /// The status of one element, whatever its codec.
    pub fn status(&self, element: &str) -> Option<&EncoderStatus> {
        self.encoders
            .iter()
            .find(|status| status.element == element)
    }

    /// Picks the encoder to use for `codec`: the user's override when it is
    /// usable, otherwise the first usable element of the plan's order.
    ///
    /// # Errors
    ///
    /// - `export.unknown_encoder` when the override names an element that is
    ///   not a catalogued encoder for `codec`.
    /// - `export.encoder_unavailable` when the override names a catalogued
    ///   encoder that this machine cannot use. The override is never silently
    ///   ignored: a user who pinned NVENC should be told it is gone rather
    ///   than handed a software encode that takes twenty times as long.
    /// - `export.no_encoder` when nothing on this machine can encode `codec`.
    pub fn select(
        &self,
        codec: VideoCodec,
        preferences: &EncoderPreferences,
    ) -> SubResult<&EncoderStatus> {
        if let Some(element) = preferences.override_for(codec) {
            let status = self
                .for_codec(codec)
                .find(|status| status.element == element)
                .ok_or_else(|| {
                    SubError::new(
                        codes::UNKNOWN_ENCODER,
                        format!("{element} is not a known {codec} encoder"),
                    )
                    .with_detail("codec", codec.as_str())
                    .with_detail("element", element)
                    .with_detail("known", encoder_names(codec))
                })?;
            if !status.is_usable() {
                return Err(SubError::new(
                    codes::ENCODER_UNAVAILABLE,
                    format!("the selected {codec} encoder {element} is unavailable here"),
                )
                .with_detail("codec", codec.as_str())
                .with_detail("element", element)
                .with_detail("present", status.present)
                .with_detail("reason", status.detail.clone())
                .with_detail(
                    "available",
                    self.usable(codec)
                        .iter()
                        .map(|status| status.element.clone())
                        .collect::<Vec<_>>(),
                ));
            }
            tracing::debug!(
                codec = codec.as_str(),
                element,
                "encoder chosen by override"
            );
            return Ok(status);
        }

        let chosen = self.for_codec(codec).find(|status| status.is_usable());
        chosen.ok_or_else(|| {
            SubError::new(
                codes::NO_ENCODER,
                format!("no {codec} encoder is available on this machine"),
            )
            .with_detail("codec", codec.as_str())
            .with_detail("known", encoder_names(codec))
        })
    }

    /// True when at least one encoder for `codec` is usable.
    pub fn can_encode(&self, codec: VideoCodec) -> bool {
        self.for_codec(codec).any(EncoderStatus::is_usable)
    }

    /// The probe as JSON, which is what `subordinate-cli diag` prints.
    ///
    /// # Errors
    ///
    /// `export.probe_failed` if the result cannot be serialised, which cannot
    /// happen for the shapes above but is reported rather than panicked on.
    pub fn to_json(&self) -> SubResult<serde_json::Value> {
        serde_json::to_value(self).map_err(|e| {
            SubError::wrap(codes::PROBE_FAILED, "encoder probe is not serialisable", &e)
        })
    }
}

/// Probes one element for real: build the factory, ask for `READY`, put it
/// back to `NULL`.
///
/// Reaching `READY` is what separates "the plugin is installed" from "the
/// hardware and driver behind it are here": `nvh264enc` registers on any
/// machine with the nvcodec plugin, and fails the state change when no NVIDIA
/// device answers.
fn probe_element(name: &str) -> ElementProbe {
    let element = match gst::ElementFactory::make(name).build() {
        Ok(element) => element,
        Err(err) => {
            tracing::debug!(element = name, error = %err, "encoder element not available");
            return ElementProbe::missing();
        }
    };
    let outcome = match element.set_state(gst::State::Ready) {
        Ok(gst::StateChangeSuccess::Async) => {
            match element.state(gst::ClockTime::from_seconds(2)).0 {
                Ok(_) => ElementProbe::ready(),
                Err(err) => ElementProbe::not_ready(err.to_string()),
            }
        }
        Ok(_) => ElementProbe::ready(),
        Err(err) => ElementProbe::not_ready(err.to_string()),
    };
    let _ = element.set_state(gst::State::Null);
    outcome
}

/// Whether `name` is an element this machine can actually run.
///
/// The same `READY` test the encoder probe uses, for the elements the probe
/// does not catalogue: muxers, parsers and audio encoders. Results are cached
/// for the life of the process, because building an element is not free and
/// the answer cannot change while the process runs.
///
/// GStreamer must already be initialised; every caller inside this crate
/// builds a pipeline first, which initialises it.
pub fn element_is_usable(name: &str) -> bool {
    static CACHE: OnceLock<Mutex<BTreeMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Ok(seen) = cache.lock()
        && let Some(&usable) = seen.get(name)
    {
        return usable;
    }
    let probe = probe_element(name);
    let usable = probe.present && probe.ready;
    if let Ok(mut seen) = cache.lock() {
        seen.insert(name.to_owned(), usable);
    }
    usable
}

#[cfg(test)]
mod tests {
    use super::{
        CATALOGUE, CODECS, ElementProbe, EncoderPreferences, EncoderProbe, EncoderVendor,
        VideoCodec, encoder_names,
    };

    fn nothing_works(_: &str) -> ElementProbe {
        ElementProbe::missing()
    }

    fn everything_works(_: &str) -> ElementProbe {
        ElementProbe::ready()
    }

    fn only(usable: &'static [&'static str]) -> impl Fn(&str) -> ElementProbe {
        move |name: &str| {
            if usable.contains(&name) {
                ElementProbe::ready()
            } else {
                ElementProbe::missing()
            }
        }
    }

    #[test]
    fn the_catalogue_is_exactly_the_elements_the_plan_names() {
        let names: Vec<&str> = CATALOGUE.iter().map(|entry| entry.element).collect();
        assert_eq!(
            names,
            vec![
                "nvh264enc",
                "nvh265enc",
                "nvav1enc",
                "vah264enc",
                "vah265enc",
                "amfh264enc",
                "amfh265enc",
                "vtenc_h264",
                "vtenc_h265",
                "mfh264enc",
                "x264enc",
                "x265enc",
            ]
        );
    }

    #[test]
    fn selection_order_per_codec_follows_the_plan() {
        assert_eq!(
            encoder_names(VideoCodec::H264),
            vec![
                "nvh264enc",
                "vah264enc",
                "amfh264enc",
                "vtenc_h264",
                "mfh264enc",
                "x264enc"
            ]
        );
        assert_eq!(
            encoder_names(VideoCodec::H265),
            vec![
                "nvh265enc",
                "vah265enc",
                "amfh265enc",
                "vtenc_h265",
                "x265enc"
            ]
        );
        assert_eq!(encoder_names(VideoCodec::Av1), vec!["nvav1enc"]);
    }

    #[test]
    fn only_software_encoders_are_reported_as_cpu() {
        for entry in CATALOGUE {
            assert_eq!(
                entry.vendor.is_hardware(),
                entry.vendor != EncoderVendor::Software,
                "{} is misclassified",
                entry.element
            );
        }
    }

    #[test]
    fn a_bare_machine_can_encode_nothing_and_says_so_per_codec() {
        let probe = EncoderProbe::from_probe("linux", &nothing_works);
        for codec in CODECS {
            assert!(!probe.can_encode(codec));
            assert!(probe.usable(codec).is_empty());
            let err = probe
                .select(codec, &EncoderPreferences::new())
                .expect_err("nothing can encode");
            assert_eq!(err.code.as_str(), "export.no_encoder");
            assert_eq!(err.details["codec"], codec.as_str());
        }
        assert!(probe.encoders.iter().all(|status| !status.present));
    }

    #[test]
    fn hardware_wins_over_software_when_both_are_ready() {
        let probe = EncoderProbe::from_probe("linux", &everything_works);
        let prefs = EncoderPreferences::new();
        assert_eq!(
            probe.select(VideoCodec::H264, &prefs).unwrap().element,
            "nvh264enc"
        );
        assert_eq!(
            probe.select(VideoCodec::H265, &prefs).unwrap().element,
            "nvh265enc"
        );
        assert_eq!(
            probe.select(VideoCodec::Av1, &prefs).unwrap().element,
            "nvav1enc"
        );
        assert_eq!(probe.usable(VideoCodec::H264).len(), 6);
    }

    #[test]
    fn selection_falls_through_the_order_to_the_first_usable_encoder() {
        let probe = EncoderProbe::from_probe("linux", &only(&["vah264enc", "x264enc", "x265enc"]));
        let prefs = EncoderPreferences::new();
        assert_eq!(
            probe.select(VideoCodec::H264, &prefs).unwrap().element,
            "vah264enc"
        );
        assert_eq!(
            probe.select(VideoCodec::H265, &prefs).unwrap().element,
            "x265enc"
        );
        assert!(probe.select(VideoCodec::Av1, &prefs).is_err());
    }

    #[test]
    fn an_element_that_registers_but_cannot_reach_ready_is_not_usable() {
        let probe = EncoderProbe::from_probe("linux", &|name: &str| match name {
            "nvh264enc" => ElementProbe::not_ready("no NVIDIA device"),
            "x264enc" => ElementProbe::ready(),
            _ => ElementProbe::missing(),
        });
        let nvenc = probe.status("nvh264enc").expect("nvenc catalogued");
        assert!(nvenc.present, "the factory registered");
        assert!(!nvenc.ready, "but the element never reached READY");
        assert!(!nvenc.is_usable());
        assert!(nvenc.summary().contains("no NVIDIA device"));
        assert_eq!(
            probe
                .select(VideoCodec::H264, &EncoderPreferences::new())
                .unwrap()
                .element,
            "x264enc"
        );
    }

    #[test]
    fn an_override_beats_the_order() {
        let probe = EncoderProbe::from_probe("linux", &everything_works);
        let mut prefs = EncoderPreferences::new();
        prefs.set_override(VideoCodec::H264, "x264enc").unwrap();
        assert_eq!(
            probe.select(VideoCodec::H264, &prefs).unwrap().element,
            "x264enc"
        );
        assert_eq!(
            probe.select(VideoCodec::H265, &prefs).unwrap().element,
            "nvh265enc",
            "other codecs keep the plan order"
        );
        assert_eq!(
            prefs.clear_override(VideoCodec::H264).as_deref(),
            Some("x264enc")
        );
        assert_eq!(
            probe.select(VideoCodec::H264, &prefs).unwrap().element,
            "nvh264enc"
        );
    }

    #[test]
    fn an_override_for_a_codec_it_does_not_encode_is_rejected_when_it_is_set() {
        let mut prefs = EncoderPreferences::new();
        let err = prefs
            .set_override(VideoCodec::H265, "x264enc")
            .expect_err("x264enc does not encode H.265");
        assert_eq!(err.code.as_str(), "export.unknown_encoder");
        assert_eq!(err.details["element"], "x264enc");
        assert!(prefs.is_empty());
        assert!(prefs.set_override(VideoCodec::H265, "x265enc").is_ok());
        assert_eq!(prefs.override_for(VideoCodec::H265), Some("x265enc"));
    }

    #[test]
    fn an_unavailable_override_is_reported_rather_than_silently_replaced() {
        let probe = EncoderProbe::from_probe("linux", &only(&["x264enc"]));
        let mut prefs = EncoderPreferences::new();
        prefs.set_override(VideoCodec::H264, "nvh264enc").unwrap();
        let err = probe
            .select(VideoCodec::H264, &prefs)
            .expect_err("the pinned encoder is missing");
        assert_eq!(err.code.as_str(), "export.encoder_unavailable");
        assert_eq!(err.details["element"], "nvh264enc");
        assert_eq!(err.details["available"], serde_json::json!(["x264enc"]));
    }

    #[test]
    fn preferences_round_trip_through_settings_json() {
        let mut prefs = EncoderPreferences::new();
        prefs.set_override(VideoCodec::H264, "vah264enc").unwrap();
        prefs.set_override(VideoCodec::Av1, "nvav1enc").unwrap();
        let json = serde_json::to_value(&prefs).expect("preferences serialise");
        assert_eq!(json["overrides"]["h264"], "vah264enc");
        assert_eq!(json["overrides"]["av1"], "nvav1enc");
        let back: EncoderPreferences = serde_json::from_value(json).expect("round-trip");
        assert_eq!(back, prefs);
        assert_eq!(
            back.overrides().collect::<Vec<_>>(),
            vec![
                (VideoCodec::H264, "vah264enc"),
                (VideoCodec::Av1, "nvav1enc")
            ]
        );
        let empty: EncoderPreferences = serde_json::from_str("{}").expect("defaults");
        assert!(empty.is_empty());
    }

    #[test]
    fn codec_names_parse_back() {
        for codec in CODECS {
            assert_eq!(VideoCodec::parse(codec.as_str()), Some(codec));
        }
        assert_eq!(VideoCodec::parse("vp9"), None);
    }

    #[test]
    fn the_json_shape_is_stable() {
        let probe = EncoderProbe::from_probe("macos", &only(&["vtenc_h264"]));
        let json = probe.to_json().expect("probe serialises");
        assert_eq!(json["platform"], "macos");
        let encoders = json["encoders"].as_array().expect("encoders are an array");
        assert_eq!(encoders.len(), CATALOGUE.len());
        assert_eq!(encoders[0]["element"], "nvh264enc");
        assert_eq!(encoders[0]["codec"], "h264");
        assert_eq!(encoders[0]["vendor"], "nvenc");
        assert_eq!(encoders[0]["hardware"], true);
        assert_eq!(encoders[0]["ready"], false);
        let round_tripped: EncoderProbe = serde_json::from_value(json).expect("round-trip");
        assert_eq!(round_tripped, probe);
    }

    #[test]
    fn this_machine_can_be_probed_and_the_answer_is_cached() {
        let first = EncoderProbe::cached().expect("GStreamer must initialise");
        let second = EncoderProbe::cached().expect("the cache never re-probes");
        assert!(
            std::ptr::eq(first, second),
            "the probe is cached for the session"
        );
        assert_eq!(first.platform, std::env::consts::OS);
        assert_eq!(first.encoders.len(), CATALOGUE.len());
        for status in &first.encoders {
            assert!(
                status.present || !status.ready,
                "{} cannot be ready while absent",
                status.element
            );
        }
    }
}
