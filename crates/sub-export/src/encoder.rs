//! Encoder capability probe and selection order (docs/PLAN.md §5.5).
//!
//! Which encoder an export should use is a per-machine question, and a
//! per-*process* one: the element may not be registered at all, or it may be
//! registered by a plugin whose driver is missing, or it may start perfectly
//! and then refuse the first caps it is given because it cannot open a session
//! here (TASK-146). The probe answers all three the only way that is honest —
//! by encoding one real frame with every catalogued encoder — and selection
//! then walks the plan's order per codec and takes the first encoder that
//! managed it.
//!
//! The scan costs a frame of encoding per element, so it is cached for the
//! life of the process ([`EncoderProbe::cached`]) and shown in the diagnostics
//! panel next to the registry report from `sub_media::HardwareDiagnostics`.
//! Because the answer can depend on what else the process has already opened,
//! a caller that is about to export should let the probe run at the point the
//! export would: after the GPU device exists, not before.
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
use std::time::{Duration, Instant};

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
    /// Software: `x264enc`, `x265enc` and the AV1 encoders.
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
///
/// Every codec is catalogued for every backend that has an element for it, so
/// an encoder a machine really carries can always be pinned: without an entry
/// [`EncoderPreferences::set_override`] refuses the element outright, which is
/// what kept AMF's AV1 encoder and Media Foundation's HEVC encoder untestable
/// (TASK-143). AV1 has three software encoders rather than one because no
/// single one of them is in a stock install everywhere: `svtav1enc` first
/// because it is the fastest of the three at a given quality.
///
/// NVENC has three element families in GStreamer 1.28 and the catalogue knows
/// all of them: `nvh264enc` drives it through CUDA, `nvd3d11h264enc` through a
/// Direct3D 11 device, and `nvautogpuh264enc` picks whichever the pipeline is
/// already using. The CUDA one is first because it is the one that exists on
/// every platform; the Direct3D ones exist only on Windows (TASK-146).
const CATALOGUE: &[Candidate] = &[
    candidate("nvh264enc", VideoCodec::H264, EncoderVendor::Nvenc),
    candidate("nvh265enc", VideoCodec::H265, EncoderVendor::Nvenc),
    candidate("nvav1enc", VideoCodec::Av1, EncoderVendor::Nvenc),
    candidate("nvd3d11h264enc", VideoCodec::H264, EncoderVendor::Nvenc),
    candidate("nvd3d11h265enc", VideoCodec::H265, EncoderVendor::Nvenc),
    candidate("nvautogpuh264enc", VideoCodec::H264, EncoderVendor::Nvenc),
    candidate("nvautogpuh265enc", VideoCodec::H265, EncoderVendor::Nvenc),
    candidate("vah264enc", VideoCodec::H264, EncoderVendor::Va),
    candidate("vah265enc", VideoCodec::H265, EncoderVendor::Va),
    candidate("vaav1enc", VideoCodec::Av1, EncoderVendor::Va),
    candidate("amfh264enc", VideoCodec::H264, EncoderVendor::Amf),
    candidate("amfh265enc", VideoCodec::H265, EncoderVendor::Amf),
    candidate("amfav1enc", VideoCodec::Av1, EncoderVendor::Amf),
    candidate("vtenc_h264", VideoCodec::H264, EncoderVendor::VideoToolbox),
    candidate("vtenc_h265", VideoCodec::H265, EncoderVendor::VideoToolbox),
    candidate(
        "mfh264enc",
        VideoCodec::H264,
        EncoderVendor::MediaFoundation,
    ),
    candidate(
        "mfh265enc",
        VideoCodec::H265,
        EncoderVendor::MediaFoundation,
    ),
    candidate("x264enc", VideoCodec::H264, EncoderVendor::Software),
    candidate("x265enc", VideoCodec::H265, EncoderVendor::Software),
    candidate("svtav1enc", VideoCodec::Av1, EncoderVendor::Software),
    candidate("av1enc", VideoCodec::Av1, EncoderVendor::Software),
    candidate("rav1enc", VideoCodec::Av1, EncoderVendor::Software),
];

/// What probing one element found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ElementProbe {
    /// Whether the element factory is registered and could be instantiated.
    pub present: bool,
    /// Whether the element encoded a frame here.
    pub ready: bool,
    /// Whether this machine has ranked the factory `NONE`.
    pub deranked: bool,
    /// Why it did not, when it did not: the GStreamer failure, rendered.
    pub detail: Option<String>,
}

impl ElementProbe {
    /// The element is not registered on this machine.
    pub fn missing() -> Self {
        Self::default()
    }

    /// The element is registered and encoded a frame here.
    pub fn ready() -> Self {
        Self {
            present: true,
            ready: true,
            deranked: false,
            detail: None,
        }
    }

    /// The element is registered but could not reach `READY`.
    pub fn not_ready(detail: impl Into<String>) -> Self {
        Self {
            present: true,
            ready: false,
            deranked: false,
            detail: Some(detail.into()),
        }
    }

    /// The same result, marked as an element this machine ranks `NONE`.
    ///
    /// The rank is recorded rather than folded into `ready`, because the two
    /// answer different questions: `ready` is "can this machine run it", the
    /// rank is "should anything plug it without being asked".
    #[must_use]
    pub fn deranked(mut self) -> Self {
        self.deranked = true;
        if self.detail.is_none() {
            self.detail = Some(DERANKED_DETAIL.to_owned());
        }
        self
    }
}

/// What the probe says about an element this machine ranks `NONE`.
const DERANKED_DETAIL: &str = "the element is ranked NONE on this machine";

/// What the probe found about one catalogued encoder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "a serialised report of four independent yes/no facts about one \
              element; collapsing them into an enum would lose the ones that \
              are true at the same time"
)]
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
    /// Whether the element encoded a frame here, which is what makes it usable.
    pub ready: bool,
    /// Whether this machine ranks the factory `NONE`. Such an element is kept
    /// out of the automatic order but still honoured when it is pinned.
    #[serde(default)]
    pub deranked: bool,
    /// Why it is unusable, when it is: the rendered GStreamer failure.
    pub detail: Option<String>,
}

impl EncoderStatus {
    /// True when the automatic selection order may pick this encoder.
    ///
    /// A machine that ranks a factory `NONE` is saying "never plug this
    /// without being asked", so a deranked element is never usable here even
    /// when it runs: see [`EncoderStatus::is_pinnable`].
    pub fn is_usable(&self) -> bool {
        self.present && self.ready && !self.deranked
    }

    /// True when this encoder works here and so may be used once something
    /// names it explicitly.
    ///
    /// Naming an element *is* the decision a `NONE` rank exists to withhold:
    /// every VA-API encoder ships ranked `NONE` by design, and `--encoder
    /// vah264enc` is a user who has already chosen it.
    pub fn is_pinnable(&self) -> bool {
        self.present && self.ready
    }

    /// A one-line description for the diagnostics panel.
    pub fn summary(&self) -> String {
        let state = if self.ready && self.deranked {
            "ready, ranked NONE (used only when pinned)".to_owned()
        } else if self.ready {
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
    /// one frame with it.
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
                    deranked: found.deranked,
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
    ///   than handed a software encode that takes twenty times as long. An
    ///   element that runs here but is ranked `NONE` is not such a case: the
    ///   rank only keeps it out of the automatic order (every VA-API encoder
    ///   ships ranked `NONE`), and naming it is the choice the rank defers.
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
            if !status.is_pinnable() {
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
                deranked = status.deranked,
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
///
/// The rank is read too, but only recorded: a `NONE` rank keeps the element
/// out of the automatic order without hiding whether it actually works, so an
/// explicitly pinned encoder can still be plugged.
fn probe_element(name: &str) -> ElementProbe {
    let probe = probe_ready(name);
    if probe.present && is_deranked(name) {
        tracing::debug!(
            element = name,
            ready = probe.ready,
            "element deranked to NONE, keeping it out of the automatic order"
        );
        return probe.deranked();
    }
    probe
}

/// The canvas the readiness probe encodes.
///
/// Above every catalogued encoder's minimum — NVENC on a T4 refuses anything
/// narrower than 129 pixels — and small enough that a frame of it costs
/// nothing anywhere.
const PROBE_CANVAS: (u32, u32) = (640, 480);

/// How long one element is given to encode that frame before it is written
/// off as unusable here.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether `name` can actually encode on this machine, in this process.
///
/// This encodes one real frame rather than driving the element to `READY`,
/// because `READY` is not the question an export needs answered. A hardware
/// encoder opens its encode session when it is given caps, not when it is
/// started, and that is where it fails: on the Windows GPU runner every NVENC
/// element in the catalogue reaches `READY` and then answers
/// `NV_ENC_ERR_INVALID_VERSION` to `NvEncOpenEncodeSessionEx` and rejects the
/// caps, which the old probe reported as `ready` and the export discovered a
/// frame later (TASK-146). One frame through `videotestsrc ! videoconvert !
/// <encoder> ! fakesink` asks the question the export is about to ask.
///
/// The pipeline is torn down before the answer is returned, so nothing the
/// probe opened is still held when the export builds its own.
fn probe_ready(name: &str) -> ElementProbe {
    let (width, height) = PROBE_CANVAS;
    match can_encode(name, width, height) {
        Ok(()) => ElementProbe::ready(),
        Err(EncodeRefusal::Missing) => {
            tracing::debug!(element = name, "encoder element not available");
            ElementProbe::missing()
        }
        Err(EncodeRefusal::Refused(reason)) => ElementProbe::not_ready(reason),
    }
}

/// Why a one-frame encode did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeRefusal {
    /// The element is not registered on this machine.
    Missing,
    /// It is registered, and this is what it said.
    Refused(String),
}

impl std::fmt::Display for EncodeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("the element is not installed"),
            Self::Refused(reason) => f.write_str(reason),
        }
    }
}

/// Encodes one `width` x `height` frame with `name`, here and now.
///
/// This is the only question that matters before an export, and it has to be
/// asked at the canvas the export will use: on the Windows GPU runner every
/// NVENC element encodes 640x480 happily and then refuses to open a session
/// for 1920x1080 in the same process, seconds later (TASK-146). One frame
/// costs a few tens of milliseconds and buys an export that starts with an
/// encoder that has just proved itself.
///
/// # Errors
///
/// [`EncodeRefusal::Missing`] when the element is not registered, and
/// [`EncodeRefusal::Refused`] carrying the element's own reason otherwise.
pub fn can_encode(name: &str, width: u32, height: u32) -> Result<(), EncodeRefusal> {
    if gst::ElementFactory::find(name).is_none() {
        return Err(EncodeRefusal::Missing);
    }
    let description = format!(
        "videotestsrc num-buffers=1 ! video/x-raw,width={width},height={height},framerate=25/1 \
         ! videoconvert ! {name} name=probe ! fakesink sync=false"
    );
    let pipeline = match gst::parse::launch(&description) {
        Ok(pipeline) => pipeline,
        Err(err) => return Err(EncodeRefusal::Refused(err.to_string())),
    };
    let outcome = run_probe_pipeline(&pipeline, name);
    let _ = pipeline.set_state(gst::State::Null);
    outcome
}

/// Runs one probe pipeline to end of stream, or says what stopped it.
fn run_probe_pipeline(pipeline: &gst::Element, name: &str) -> Result<(), EncodeRefusal> {
    if let Err(err) = pipeline.set_state(gst::State::Playing) {
        return Err(EncodeRefusal::Refused(err.to_string()));
    }
    let Some(bus) = pipeline.bus() else {
        return Err(EncodeRefusal::Refused(
            "the probe pipeline has no bus".to_owned(),
        ));
    };
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            tracing::debug!(element = name, "encoder did not encode a frame in time");
            return Err(EncodeRefusal::Refused(format!(
                "{name} did not encode a frame within {}s",
                PROBE_TIMEOUT.as_secs()
            )));
        }
        let nanos = u64::try_from(left.as_nanos()).unwrap_or(u64::MAX);
        let message = bus.timed_pop_filtered(
            gst::ClockTime::from_nseconds(nanos),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        let Some(message) = message else {
            continue;
        };
        match message.view() {
            gst::MessageView::Eos(_) => return Ok(()),
            gst::MessageView::Error(err) => {
                let reason = err.error().to_string();
                tracing::debug!(element = name, reason, "encoder cannot encode here");
                return Err(EncodeRefusal::Refused(reason));
            }
            _ => continue,
        }
    }
}

/// Whether `name` has been deranked to `NONE`, by
/// `GST_PLUGIN_FEATURE_RANK` or by the application itself.
///
/// Rank `NONE` is how a machine says "never plug this unasked": it is what
/// keeps a decoder or encoder that registers but cannot work here out of every
/// autoplugged pipeline. Selection picks its elements by name rather than by
/// autoplugging, so it has to honour that answer itself -- otherwise the one
/// escape hatch a user (or a GPU-less CI runner) has does not reach the
/// exporter. It is only the *automatic* order that honours it: an explicitly
/// pinned element is the very decision the rank withholds. An element whose
/// factory is gone counts as not deranked; the probe then reports it missing.
fn is_deranked(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some_and(|factory| factory.rank() == gst::Rank::NONE)
}

/// Whether `name` builds and reaches `READY` on this machine.
fn starts_to_ready(name: &str) -> bool {
    let Ok(element) = gst::ElementFactory::make(name).build() else {
        tracing::debug!(element = name, "element not available");
        return false;
    };
    let started = match element.set_state(gst::State::Ready) {
        Ok(gst::StateChangeSuccess::Async) => element
            .state(gst::ClockTime::from_seconds(2))
            .0
            .is_ok(),
        Ok(_) => true,
        Err(err) => {
            tracing::debug!(element = name, error = %err, "element would not start");
            false
        }
    };
    let _ = element.set_state(gst::State::Null);
    started
}

/// Whether `name` is an element this machine can actually run.
///
/// For the elements the encoder catalogue does not cover — muxers, parsers and
/// audio encoders — the question is only whether this machine has one that
/// starts: they are not video encoders and cannot be asked to encode a frame,
/// so this builds the element, drives it to `READY` and puts it back. Results
/// are cached for the life of the process, because building an element is not
/// free and the answer cannot change while the process runs.
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
    let usable = starts_to_ready(name) && !is_deranked(name);
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
                "nvd3d11h264enc",
                "nvd3d11h265enc",
                "nvautogpuh264enc",
                "nvautogpuh265enc",
                "vah264enc",
                "vah265enc",
                "vaav1enc",
                "amfh264enc",
                "amfh265enc",
                "amfav1enc",
                "vtenc_h264",
                "vtenc_h265",
                "mfh264enc",
                "mfh265enc",
                "x264enc",
                "x265enc",
                "svtav1enc",
                "av1enc",
                "rav1enc",
            ]
        );
    }

    #[test]
    fn selection_order_per_codec_follows_the_plan() {
        assert_eq!(
            encoder_names(VideoCodec::H264),
            vec![
                "nvh264enc",
                "nvd3d11h264enc",
                "nvautogpuh264enc",
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
                "nvd3d11h265enc",
                "nvautogpuh265enc",
                "vah265enc",
                "amfh265enc",
                "vtenc_h265",
                "mfh265enc",
                "x265enc"
            ]
        );
        assert_eq!(
            encoder_names(VideoCodec::Av1),
            vec![
                "nvav1enc",
                "vaav1enc",
                "amfav1enc",
                "svtav1enc",
                "av1enc",
                "rav1enc"
            ]
        );
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
        assert_eq!(probe.usable(VideoCodec::H264).len(), 8);
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

    /// A machine says "never plug this unasked" by ranking a factory NONE, and
    /// selection picks by name rather than by autoplugging, so the probe has
    /// to read that rank itself. The GPU-less Windows CI runners rank
    /// `mfh264enc` NONE for exactly this reason. The element is still probed
    /// for real, so a pinned encoder can be told apart from a broken one.
    #[test]
    fn an_element_ranked_none_is_reported_deranked_but_still_probed() {
        use gstreamer::prelude::PluginFeatureExtManual;

        let _ = gstreamer::init();
        // A real video encoder, because the probe now encodes a frame with
        // what it is given: anything else fails by construction.
        let Some(factory) = gstreamer::ElementFactory::find("x264enc") else {
            eprintln!("skipping: this machine has no x264enc");
            return;
        };
        let rank = factory.rank();
        factory.set_rank(gstreamer::Rank::NONE);
        let probe = super::probe_element("x264enc");
        factory.set_rank(rank);

        assert!(probe.present, "the factory is registered");
        assert!(probe.deranked, "and this machine ranked it NONE");
        assert!(probe.ready, "the element itself still runs here");
        assert!(
            probe.detail.is_some_and(|why| why.contains("NONE")),
            "and the reason says why it is kept out of the order"
        );
    }

    /// The catalogue order is the automatic order, and a deranked element is
    /// not in it: this is what keeps `mfh264enc` off the GPU-less Windows
    /// runners and `vah264enc` out of an unasked-for export.
    #[test]
    fn automatic_selection_skips_a_deranked_encoder() {
        let probe = EncoderProbe::from_probe("linux", &|name: &str| match name {
            "vah264enc" => ElementProbe::ready().deranked(),
            "x264enc" => ElementProbe::ready(),
            _ => ElementProbe::missing(),
        });
        let va = probe.status("vah264enc").expect("vah264enc catalogued");
        assert!(va.present && va.ready, "the element runs here");
        assert!(!va.is_usable(), "but the automatic order must not pick it");
        assert!(va.is_pinnable(), "naming it is still allowed");
        assert!(va.summary().contains("ranked NONE"), "{}", va.summary());
        assert_eq!(
            probe
                .select(VideoCodec::H264, &EncoderPreferences::new())
                .unwrap()
                .element,
            "x264enc"
        );
        assert_eq!(
            probe
                .usable(VideoCodec::H264)
                .iter()
                .map(|status| status.element.as_str())
                .collect::<Vec<_>>(),
            vec!["x264enc"]
        );
    }

    /// Every VA-API encoder ships ranked NONE, so pinning one has to work
    /// without `GST_PLUGIN_FEATURE_RANK` (TASK-134).
    #[test]
    fn a_pinned_encoder_is_used_even_when_it_is_ranked_none() {
        let probe = EncoderProbe::from_probe("linux", &|name: &str| match name {
            "vah264enc" => ElementProbe::ready().deranked(),
            "x264enc" => ElementProbe::ready(),
            _ => ElementProbe::missing(),
        });
        let mut prefs = EncoderPreferences::new();
        prefs.set_override(VideoCodec::H264, "vah264enc").unwrap();
        assert_eq!(
            probe.select(VideoCodec::H264, &prefs).unwrap().element,
            "vah264enc"
        );
    }

    /// Deranking is not a way to make a broken element usable: an element that
    /// cannot reach READY is still refused when it is pinned.
    #[test]
    fn a_pinned_encoder_that_cannot_run_is_still_refused() {
        let probe = EncoderProbe::from_probe("linux", &|name: &str| match name {
            "vah264enc" => ElementProbe::not_ready("no VA driver").deranked(),
            "x264enc" => ElementProbe::ready(),
            _ => ElementProbe::missing(),
        });
        let mut prefs = EncoderPreferences::new();
        prefs.set_override(VideoCodec::H264, "vah264enc").unwrap();
        let err = probe.select(VideoCodec::H264, &prefs).unwrap_err();
        assert_eq!(err.code, crate::codes::ENCODER_UNAVAILABLE);
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
        assert_eq!(encoders[0]["deranked"], false);
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
