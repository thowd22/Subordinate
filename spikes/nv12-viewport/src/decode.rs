//! The GStreamer half of the spike: decode a file into NV12 buffers, and do
//! it on the GPU wherever the machine can.
//!
//! `decodebin3` picks a decoder by element rank, and on most Linux boxes the
//! software `avdec_h264` outranks `vah264dec`, so the CPU decodes unless the
//! application says otherwise. The old `autoplug-sort` signal is *not* the
//! answer: `uridecodebin3` and `decodebin3` do not have it (the spike found
//! this the hard way — connecting to it panics with "Signal 'autoplug-sort'
//! of type `GstURIDecodeBin3` not found"). What decodebin3 honours is the
//! registry rank, which is what `GST_PLUGIN_FEATURE_RANK` sets from outside
//! and what [`prefer_hardware_decoders`] sets from inside the process.
//!
//! Everything about the ordering is a pure function of factory names, so it
//! is unit-tested on machines that have none of these decoders installed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use gstreamer as gst;
use gstreamer::glib::translate::IntoGlib as _;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use gstreamer_video::VideoInfo;
use sub_core::{ResultExt as _, SubError, SubResult};
use sub_time::{Rational, RationalTime};

use crate::codes;
use crate::frames::{DecodedFrame, FrameSlot};
use crate::nv12::Nv12Geometry;

/// The rate GStreamer timestamps are exact at: one unit per nanosecond.
/// PTS therefore crosses into `sub-time` with no rounding and no float.
pub const NANOSECONDS: Rational = match Rational::new(1_000_000_000, 1) {
    Some(rate) => rate,
    None => unreachable!(),
};

/// Which silicon a decoder factory runs on, most wanted first.
///
/// The order of the variants *is* the preference: `Ord` is what sorts the
/// autoplug list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DecoderKind {
    /// A CPU decoder: `avdec_h264`, `openh264dec`, `libde265dec`.
    Software,
    /// Something unrecognised. Ranked above software only when its factory
    /// name says nothing, so it never displaces a known hardware decoder.
    Unknown,
    /// Intel Media SDK / oneVPL.
    QuickSync,
    /// Direct3D 11 video, the older Windows path.
    D3d11,
    /// Direct3D 12 video, what the plan targets on Windows.
    D3d12,
    /// Apple `VideoToolbox`.
    VideoToolbox,
    /// Mesa VA-API, the plan's Linux path on Intel and AMD.
    Va,
    /// NVIDIA NVDEC, the plan's Linux path on NVIDIA.
    Nvdec,
}

impl DecoderKind {
    /// True when the decoder runs on a fixed-function video engine.
    pub fn is_hardware(self) -> bool {
        !matches!(self, Self::Software | Self::Unknown)
    }

    /// The word this kind is logged and reported as.
    pub fn label(self) -> &'static str {
        match self {
            Self::Software => "software",
            Self::Unknown => "unknown",
            Self::QuickSync => "quicksync",
            Self::D3d11 => "d3d11",
            Self::D3d12 => "d3d12",
            Self::VideoToolbox => "videotoolbox",
            Self::Va => "va",
            Self::Nvdec => "nvdec",
        }
    }
}

/// Classify a decoder by its GStreamer factory name.
///
/// Names are matched by prefix because every family spells its codecs the
/// same way: `vah264dec`, `vah265dec`, `vavp9dec` are all VA-API.
pub fn decoder_kind(factory_name: &str) -> DecoderKind {
    // Software decoders first: `avdec_*` would otherwise be caught by nothing
    // and land in `Unknown`, which outranks it.
    if factory_name.starts_with("avdec_")
        || factory_name.starts_with("openh264")
        || factory_name.starts_with("libde265")
        || factory_name.starts_with("vp8dec")
        || factory_name.starts_with("vp9dec")
        || factory_name.starts_with("dav1d")
    {
        return DecoderKind::Software;
    }
    if factory_name.starts_with("nv") {
        return DecoderKind::Nvdec;
    }
    if factory_name.starts_with("va") {
        return DecoderKind::Va;
    }
    if factory_name.starts_with("vtdec") {
        return DecoderKind::VideoToolbox;
    }
    if factory_name.starts_with("d3d12") {
        return DecoderKind::D3d12;
    }
    if factory_name.starts_with("d3d11") {
        return DecoderKind::D3d11;
    }
    if factory_name.starts_with("msdk") || factory_name.starts_with("qsv") {
        return DecoderKind::QuickSync;
    }
    DecoderKind::Unknown
}

/// Order two candidate factories, most wanted first.
///
/// Hardware kind decides; within one kind GStreamer's own rank decides; ties
/// keep the registry order so the choice is deterministic.
pub fn compare_candidates(
    (left_name, left_rank): (&str, u32),
    (right_name, right_rank): (&str, u32),
) -> std::cmp::Ordering {
    decoder_kind(right_name)
        .cmp(&decoder_kind(left_name))
        .then(right_rank.cmp(&left_rank))
}

/// Sort candidate `(factory name, rank)` pairs into the order the pipeline
/// should try them in.
pub fn sort_candidates(candidates: &mut [(&str, u32)]) {
    candidates.sort_by(|left, right| compare_candidates(*left, *right));
}

/// The registry rank a decoder of `kind` should be given so decodebin3 tries
/// the hardware families in the order [`DecoderKind`] declares.
///
/// Software and unrecognised decoders keep whatever rank they were shipped
/// with, so nothing is demoted and a machine with no hardware decoder behaves
/// exactly as before.
pub fn boosted_rank(kind: DecoderKind) -> Option<gst::Rank> {
    if !kind.is_hardware() {
        return None;
    }
    // PRIMARY is 256 and avdec_h264 sits exactly there, so every hardware
    // decoder must land above it; the offset spreads the families apart in
    // the order of the enum.
    let offset = match kind {
        DecoderKind::QuickSync => 1,
        DecoderKind::D3d11 => 2,
        DecoderKind::D3d12 => 3,
        DecoderKind::VideoToolbox => 4,
        DecoderKind::Va => 5,
        DecoderKind::Nvdec => 6,
        DecoderKind::Software | DecoderKind::Unknown => return None,
    };
    Some(gst::Rank::PRIMARY + offset)
}

/// Raise the registry rank of every hardware video decoder this installation
/// has, so `decodebin3` autoplugs one in preference to `avdec_*`.
///
/// This is process-local: it edits the in-memory registry this process
/// loaded, not anything on disk. Returns the boosted decoders in the order
/// they are now preferred, which is what the window reports.
///
/// # Errors
///
/// [`codes::INIT_FAILED`] when GStreamer will not initialise.
pub fn prefer_hardware_decoders() -> SubResult<Vec<String>> {
    gst::init().sub_context(codes::INIT_FAILED, "GStreamer could not initialise")?;
    let factories = gst::ElementFactory::factories_with_type(
        gst::ElementFactoryType::DECODER | gst::ElementFactoryType::MEDIA_VIDEO,
        gst::Rank::NONE,
    );
    let mut boosted: Vec<(String, u32)> = Vec::new();
    for factory in factories {
        let name = factory.name().to_string();
        let Some(rank) = boosted_rank(decoder_kind(&name)) else {
            continue;
        };
        factory.set_rank(rank);
        boosted.push((name, rank_of(&factory)));
    }
    let mut view: Vec<(&str, u32)> = boosted
        .iter()
        .map(|(name, rank)| (name.as_str(), *rank))
        .collect();
    sort_candidates(&mut view);
    let ordered: Vec<String> = view.iter().map(|(name, _)| (*name).to_owned()).collect();
    tracing::info!(?ordered, "hardware decoders promoted above software ones");
    Ok(ordered)
}

/// The caps the appsink accepts: system-memory NV12 and nothing else, so a
/// hardware decoder's own memory is downloaded once by `videoconvert` rather
/// than surprising the upload path with a DMA-BUF it cannot read.
fn nv12_caps() -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("format", "NV12")
        .build()
}

/// Read the plane geometry out of a negotiated caps structure.
///
/// # Errors
///
/// [`codes::BAD_CAPS`] when the caps are not NV12 video, or carry a size the
/// upload path cannot describe.
pub fn geometry_from_caps(caps: &gst::CapsRef) -> SubResult<Nv12Geometry> {
    let info = VideoInfo::from_caps(caps)
        .sub_context_with(codes::BAD_CAPS, || format!("caps {caps} are not video"))?;
    let y_stride = u32::try_from(info.stride()[0].max(0))
        .sub_context(codes::BAD_CAPS, "luma stride does not fit in u32")?;
    let uv_stride = u32::try_from(info.stride()[1].max(0))
        .sub_context(codes::BAD_CAPS, "chroma stride does not fit in u32")?;
    Nv12Geometry::new(info.width(), info.height(), y_stride, uv_stride)
}

/// How the running pipeline is getting on.
#[derive(Debug, Default)]
pub struct DecodeStats {
    /// Frames the appsink handed over.
    pub delivered: AtomicU64,
    /// Frames the newest-wins slot threw away because the UI was behind.
    pub dropped: AtomicU64,
}

impl DecodeStats {
    /// Frames delivered so far.
    pub fn delivered(&self) -> u64 {
        self.delivered.load(Ordering::Relaxed)
    }

    /// Frames dropped so far.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// A running decode pipeline feeding a [`FrameSlot`].
#[derive(Debug)]
pub struct Decoder {
    pipeline: gst::Pipeline,
    stats: Arc<DecodeStats>,
    hardware_choice: Arc<std::sync::Mutex<Option<String>>>,
    preferred: Vec<String>,
}

impl Decoder {
    /// Build and start a pipeline decoding `uri` into `slot`.
    ///
    /// # Errors
    ///
    /// [`codes::PIPELINE_BUILD`] when an element is missing from this
    /// installation, and [`codes::PIPELINE_START`] when the pipeline refuses
    /// to play.
    pub fn start(uri: &str, slot: Arc<FrameSlot>) -> SubResult<Self> {
        let preferred = prefer_hardware_decoders()?;

        let pipeline = gst::Pipeline::with_name("spike-nv12");
        let source = make_element("uridecodebin3")?;
        source.set_property("uri", uri);
        let convert = make_element("videoconvert")?;
        let sink = make_element("appsink")?;

        let appsink = sink
            .clone()
            .dynamic_cast::<AppSink>()
            .map_err(|_| SubError::new(codes::PIPELINE_BUILD, "appsink is not an AppSink"))?;
        appsink.set_caps(Some(&nv12_caps()));
        appsink.set_max_buffers(2);
        appsink.set_drop(true);
        appsink.set_sync(true);

        pipeline
            .add_many([&source, &convert, &sink])
            .sub_context(codes::PIPELINE_BUILD, "elements could not be added")?;
        convert.link(&sink).sub_context(
            codes::PIPELINE_BUILD,
            "videoconvert could not reach appsink",
        )?;

        let hardware_choice = Arc::new(std::sync::Mutex::new(None));
        watch_chosen_decoder(&pipeline, &hardware_choice);
        link_decoded_video(&source, &convert);

        let stats = Arc::new(DecodeStats::default());
        install_appsink_callbacks(&appsink, slot, Arc::clone(&stats));

        pipeline
            .set_state(gst::State::Playing)
            .sub_context_with(codes::PIPELINE_START, || {
                format!("pipeline for {uri} would not play")
            })?;

        Ok(Self {
            pipeline,
            stats,
            hardware_choice,
            preferred,
        })
    }

    /// Counters for the running pipeline.
    pub fn stats(&self) -> &Arc<DecodeStats> {
        &self.stats
    }

    /// The hardware decoders promoted for this run, most preferred first.
    pub fn preferred_decoders(&self) -> &[String] {
        &self.preferred
    }

    /// The decoder element the pipeline actually instantiated, once it has
    /// negotiated. `None` until then.
    pub fn chosen_decoder(&self) -> Option<String> {
        self.hardware_choice
            .lock()
            .ok()
            .and_then(|choice| choice.clone())
    }

    /// Drain the bus, returning the first error posted so far.
    pub fn take_error(&self) -> Option<SubError> {
        let bus = self.pipeline.bus()?;
        while let Some(message) = bus.pop() {
            if let gst::MessageView::Error(error) = message.view() {
                return Some(
                    SubError::new(codes::PIPELINE_FAILED, error.error().to_string())
                        .with_detail("debug", error.debug().unwrap_or_default().to_string()),
                );
            }
        }
        None
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Synthesise a test clip with `videotestsrc`, so the spike can measure a
/// real decode-to-display path on a machine with no sample media.
///
/// The codec is whatever `encoder` names (`vp8enc`, `x264enc`, ...) because
/// no single encoder is present everywhere; the container is Matroska, which the
/// `matroska` plugin writes wherever GStreamer is installed at all.
///
/// # Errors
///
/// [`codes::PIPELINE_BUILD`] when an element is missing, and
/// [`codes::PIPELINE_FAILED`] when the encode does not reach end-of-stream.
pub fn make_clip(
    path: &std::path::Path,
    encoder: &str,
    width: u32,
    height: u32,
    frames: u32,
) -> SubResult<()> {
    gst::init().sub_context(codes::INIT_FAILED, "GStreamer could not initialise")?;

    let source = make_element("videotestsrc")?;
    source.set_property("num-buffers", i32::try_from(frames).unwrap_or(i32::MAX));
    let caps_filter = make_element("capsfilter")?;
    caps_filter.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", "I420")
            .field("width", i32::try_from(width).unwrap_or(i32::MAX))
            .field("height", i32::try_from(height).unwrap_or(i32::MAX))
            .field("framerate", gst::Fraction::new(25, 1))
            .build(),
    );
    let encode = make_element(encoder)?;
    // Fast, not pretty: the clip only has to decode, and a slow encode would
    // dominate the spike's runtime at 4K.
    if encode.has_property("deadline") {
        encode.set_property("deadline", 1i64);
    }
    if encode.has_property("cpu-used") {
        encode.set_property("cpu-used", 16i32);
    }
    let mux = make_element("webmmux")?;
    let sink = make_element("filesink")?;
    sink.set_property("location", path.to_string_lossy().as_ref());

    let pipeline = gst::Pipeline::with_name("spike-make-clip");
    pipeline
        .add_many([&source, &caps_filter, &encode, &mux, &sink])
        .sub_context(codes::PIPELINE_BUILD, "encode elements could not be added")?;
    gst::Element::link_many([&source, &caps_filter, &encode, &mux, &sink]).sub_context(
        codes::PIPELINE_BUILD,
        "the encode chain could not be linked",
    )?;

    pipeline
        .set_state(gst::State::Playing)
        .sub_context(codes::PIPELINE_START, "the encode pipeline would not play")?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| SubError::new(codes::PIPELINE_FAILED, "the pipeline has no bus"))?;
    let outcome = bus.timed_pop_filtered(
        gst::ClockTime::NONE,
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    let _ = pipeline.set_state(gst::State::Null);
    match outcome.as_ref().map(|message| message.view()) {
        Some(gst::MessageView::Eos(_)) => Ok(()),
        Some(gst::MessageView::Error(error)) => Err(SubError::new(
            codes::PIPELINE_FAILED,
            error.error().to_string(),
        )),
        _ => Err(SubError::new(
            codes::PIPELINE_FAILED,
            "the encode ended without end-of-stream",
        )),
    }
}

/// Instantiate one element, or say which plugin is missing.
fn make_element(factory: &str) -> SubResult<gst::Element> {
    gst::ElementFactory::make(factory)
        .build()
        .sub_context_with(codes::PIPELINE_BUILD, || {
            format!("this GStreamer installation has no {factory} element")
        })
}

/// Record which decoder element the pipeline actually instantiated.
///
/// `deep-element-added` fires for every element any nested bin adds, which is
/// how the decoder decodebin3 chose becomes observable without depending on
/// signals decodebin3 does not have.
fn watch_chosen_decoder(pipeline: &gst::Pipeline, choice: &Arc<std::sync::Mutex<Option<String>>>) {
    let choice = Arc::clone(choice);
    pipeline.connect_deep_element_added(move |_, _, element| {
        let Some(factory) = element.factory() else {
            return;
        };
        let name = factory.name().to_string();
        if !name.ends_with("dec") && !name.contains("dec_") {
            return;
        }
        let kind = decoder_kind(&name);
        tracing::info!(decoder = %name, kind = kind.label(), "decoder instantiated");
        if let Ok(mut slot) = choice.lock() {
            // Keep the best decoder seen: a hardware one must not be
            // overwritten by a parser or a later software fallback element.
            let better = slot
                .as_deref()
                .is_none_or(|current| kind > decoder_kind(current));
            if better {
                *slot = Some(name);
            }
        }
    });
}

/// A factory's GStreamer rank as a plain number, clamped at zero.
fn rank_of(factory: &gst::ElementFactory) -> u32 {
    u32::try_from(factory.rank().into_glib()).unwrap_or(0)
}

/// Link `decodebin`'s video pad to the converter when it appears.
fn link_decoded_video(source: &gst::Element, convert: &gst::Element) {
    let convert = convert.clone();
    source.connect_pad_added(move |_, pad| {
        // decodebin3 adds its pads before their caps are negotiated, so
        // `current_caps` is usually `None` here and only the pad name says
        // what the stream is. Falling back to the name is what makes the
        // link happen at all.
        let is_video = pad
            .current_caps()
            .and_then(|caps| caps.structure(0).map(|s| s.name().starts_with("video/")))
            .unwrap_or_else(|| pad.name().starts_with("video"));
        if !is_video {
            tracing::debug!(pad = %pad.name(), "ignoring a non-video pad");
            return;
        }
        let Some(target) = convert.static_pad("sink") else {
            return;
        };
        if target.is_linked() {
            return;
        }
        match pad.link(&target) {
            Ok(_) => tracing::info!(pad = %pad.name(), "decoded video linked to the converter"),
            Err(error) => tracing::error!(?error, "could not link the decoded video pad"),
        }
    });
}

/// Hand every sample to the newest-wins slot.
fn install_appsink_callbacks(appsink: &AppSink, slot: Arc<FrameSlot>, stats: Arc<DecodeStats>) {
    appsink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                match frame_from_sample(&sample) {
                    Ok(frame) => {
                        stats.delivered.fetch_add(1, Ordering::Relaxed);
                        if slot.put(frame) {
                            stats.dropped.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(gst::FlowSuccess::Ok)
                    }
                    Err(error) => {
                        tracing::error!(code = %error.code, "{}", error.message);
                        Err(gst::FlowError::Error)
                    }
                }
            })
            .build(),
    );
}

/// Copy one NV12 sample out of GStreamer's memory into owned plane buffers.
///
/// The spike copies deliberately: it is the copy whose cost the findings
/// measure, and it is what a zero-copy DMA-BUF import would remove.
///
/// # Errors
///
/// [`codes::BAD_CAPS`] for caps the upload path cannot use, and
/// [`codes::BAD_BUFFER`] for a sample with no readable buffer.
pub fn frame_from_sample(sample: &gst::Sample) -> SubResult<DecodedFrame> {
    let caps = sample
        .caps()
        .ok_or_else(|| SubError::new(codes::BAD_CAPS, "sample carries no caps"))?;
    let geometry = geometry_from_caps(caps)?;
    let buffer = sample
        .buffer()
        .ok_or_else(|| SubError::new(codes::BAD_BUFFER, "sample carries no buffer"))?;
    let map = buffer
        .map_readable()
        .sub_context(codes::BAD_BUFFER, "buffer could not be mapped for reading")?;
    let bytes = map.as_slice();
    if bytes.len() < geometry.frame_len() {
        return Err(SubError::new(
            codes::BAD_BUFFER,
            format!(
                "buffer holds {} bytes, {} needed for {}x{}",
                bytes.len(),
                geometry.frame_len(),
                geometry.width(),
                geometry.height()
            ),
        ));
    }
    let (y, uv) = bytes.split_at(geometry.y_plane_len());
    Ok(DecodedFrame {
        geometry,
        pts: pts_of(buffer),
        y: y.to_vec(),
        uv: uv[..geometry.uv_plane_len()].to_vec(),
        arrived: Instant::now(),
    })
}

/// The buffer's presentation time as an exact [`RationalTime`] in
/// nanoseconds; zero when the buffer carries none.
fn pts_of(buffer: &gst::BufferRef) -> RationalTime {
    buffer.pts().map_or_else(
        || RationalTime::zero(NANOSECONDS),
        |pts| {
            let nanos = i64::try_from(pts.nseconds()).unwrap_or(i64::MAX);
            RationalTime::new(nanos, NANOSECONDS)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{DecoderKind, boosted_rank, decoder_kind, sort_candidates};
    use gstreamer as gst;

    #[test]
    fn factory_names_map_to_their_silicon() {
        assert_eq!(decoder_kind("nvh264dec"), DecoderKind::Nvdec);
        assert_eq!(decoder_kind("nvh264sldec"), DecoderKind::Nvdec);
        assert_eq!(decoder_kind("vah264dec"), DecoderKind::Va);
        assert_eq!(decoder_kind("vavp9dec"), DecoderKind::Va);
        assert_eq!(decoder_kind("vtdec_hw"), DecoderKind::VideoToolbox);
        assert_eq!(decoder_kind("d3d12h264dec"), DecoderKind::D3d12);
        assert_eq!(decoder_kind("d3d11h264dec"), DecoderKind::D3d11);
        assert_eq!(decoder_kind("qsvh264dec"), DecoderKind::QuickSync);
        assert_eq!(decoder_kind("avdec_h264"), DecoderKind::Software);
        assert_eq!(decoder_kind("openh264dec"), DecoderKind::Software);
        assert_eq!(decoder_kind("dav1ddec"), DecoderKind::Software);
    }

    #[test]
    fn only_fixed_function_decoders_count_as_hardware() {
        assert!(DecoderKind::Nvdec.is_hardware());
        assert!(DecoderKind::Va.is_hardware());
        assert!(DecoderKind::D3d12.is_hardware());
        assert!(!DecoderKind::Software.is_hardware());
        assert!(!DecoderKind::Unknown.is_hardware());
    }

    #[test]
    fn hardware_wins_even_when_software_has_the_higher_rank() {
        // This is exactly the Linux default: avdec_h264 is PRIMARY (256),
        // vah264dec is MARGINAL (64), and without this sort the CPU decodes.
        let mut candidates = [("avdec_h264", 256), ("vah264dec", 64)];
        sort_candidates(&mut candidates);
        assert_eq!(candidates[0].0, "vah264dec");
        assert_eq!(candidates[1].0, "avdec_h264");
    }

    #[test]
    fn nvdec_outranks_va_which_outranks_software() {
        let mut candidates = [("vah264dec", 64), ("avdec_h264", 256), ("nvh264dec", 64)];
        sort_candidates(&mut candidates);
        let order: Vec<&str> = candidates.iter().map(|(name, _)| *name).collect();
        assert_eq!(order, ["nvh264dec", "vah264dec", "avdec_h264"]);
    }

    #[test]
    fn within_one_kind_the_gstreamer_rank_decides() {
        let mut candidates = [("vah265dec", 64), ("vah264dec", 128)];
        sort_candidates(&mut candidates);
        assert_eq!(candidates[0].0, "vah264dec");
    }

    #[test]
    fn an_unknown_decoder_never_displaces_a_hardware_one() {
        let mut candidates = [("mysterydec", 512), ("vah264dec", 1)];
        sort_candidates(&mut candidates);
        assert_eq!(candidates[0].0, "vah264dec");
        // ...but it is still tried before a software decoder, since it may
        // itself be a hardware decoder this list has not learned about.
        let mut candidates = [("avdec_h264", 512), ("mysterydec", 1)];
        sort_candidates(&mut candidates);
        assert_eq!(candidates[0].0, "mysterydec");
    }

    #[test]
    fn every_hardware_decoder_is_promoted_above_a_primary_software_one() {
        for kind in [
            DecoderKind::QuickSync,
            DecoderKind::D3d11,
            DecoderKind::D3d12,
            DecoderKind::VideoToolbox,
            DecoderKind::Va,
            DecoderKind::Nvdec,
        ] {
            let rank = boosted_rank(kind).expect("hardware decoders are promoted");
            assert!(
                rank > gst::Rank::PRIMARY,
                "{} must outrank avdec_h264 at PRIMARY",
                kind.label()
            );
        }
    }

    #[test]
    fn software_decoders_keep_the_rank_they_shipped_with() {
        assert_eq!(boosted_rank(DecoderKind::Software), None);
        assert_eq!(boosted_rank(DecoderKind::Unknown), None);
    }

    #[test]
    fn the_promoted_ranks_agree_with_the_preference_order() {
        let nvdec = boosted_rank(DecoderKind::Nvdec).expect("nvdec is promoted");
        let va = boosted_rank(DecoderKind::Va).expect("va is promoted");
        let d3d12 = boosted_rank(DecoderKind::D3d12).expect("d3d12 is promoted");
        assert!(nvdec > va, "nvdec is preferred over va");
        assert!(va > d3d12, "the enum order and the ranks must not disagree");
    }

    #[test]
    fn sorting_an_empty_or_single_candidate_list_is_a_no_op() {
        let mut none: [(&str, u32); 0] = [];
        sort_candidates(&mut none);
        let mut one = [("avdec_h264", 256)];
        sort_candidates(&mut one);
        assert_eq!(one[0].0, "avdec_h264");
    }
}
