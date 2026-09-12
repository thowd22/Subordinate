//! Turning a preset's quality into encoder element properties
//! (docs/PLAN.md §5.5).
//!
//! A preset says "12000 kbit/s" or "CRF 20"; an encoder element says
//! `bitrate`, `target-bitrate`, `rc-mode=constqp`, `qp-const`, `crf` or
//! `min-quantizer`, and no two vendors spell it the same way. This module is
//! the one place that mapping is written down: [`MAPPINGS`] names, per
//! catalogued element, the properties to set for an average bitrate and the
//! properties to set for a constant-quality target, and
//! [`apply_video_quality`] walks them.
//!
//! Three rules make the table safe to state even for hardware this machine
//! has never seen:
//!
//! - a property is set only when the element actually carries it, so an
//!   element whose plugin renamed a knob is warned about, not crashed on
//!   (`set_property` panics on an unknown property);
//! - an enum property is set by nick, and only by a nick the element's own
//!   enum carries, so the several spellings of "constant QP" across NVENC
//!   generations are tried in order;
//! - a numeric value is clamped to the property's own range.
//!
//! Anything that cannot be applied comes back as a warning string and is
//! logged: an export that cannot be driven at the asked-for quality still
//! writes a file, at the element's default rate control.
//!
//! CRF values are written on the H.264 scale of 0..=51 that the presets use
//! ([`MAX_CRF`]). An encoder whose quantiser range is wider — AV1's 0..=63 or
//! 0..=255 — is given the value rescaled onto its own range, so "CRF 20" means
//! roughly the same picture whichever encoder runs.

use gstreamer as gst;
use gstreamer::glib;
use gstreamer::prelude::*;

use crate::pipeline::{MAX_CRF, VideoQuality};

/// The unit a bitrate property takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitrateUnit {
    /// Kilobits per second, what most GStreamer encoders take.
    Kbps,
    /// Bits per second, what the AV1 Rust encoders and every audio encoder
    /// take.
    Bps,
}

impl BitrateUnit {
    /// `kbps` expressed in this unit.
    fn value(self, kbps: u32) -> u64 {
        match self {
            Self::Kbps => u64::from(kbps),
            Self::Bps => u64::from(kbps) * 1_000,
        }
    }
}

/// What one knob is set to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setting {
    /// An enum property, set to the first of these nicks the element's enum
    /// carries. The several spellings exist because the same mode is
    /// `constqp` on one NVENC generation and `cqp` on the next.
    Mode(&'static [&'static str]),
    /// The average bitrate, in the unit the property documents.
    Bitrate(BitrateUnit),
    /// The quality target, rescaled from the CRF scale onto `0..=max`.
    Quantiser {
        /// The encoder's own worst-quality quantiser.
        max: u32,
    },
}

/// One property to set on an encoder, with the names it may go by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Knob {
    /// Property names to try, in order; the first the element carries wins.
    names: &'static [&'static str],
    /// The value to give it.
    setting: Setting,
}

const fn knob(names: &'static [&'static str], setting: Setting) -> Knob {
    Knob { names, setting }
}

/// How one catalogued encoder is driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mapping {
    /// The element factory name, matching [`crate::encoder`]'s catalogue.
    element: &'static str,
    /// The knobs for an average bitrate.
    bitrate: &'static [Knob],
    /// The knobs for a constant-quality target; empty when the encoder has no
    /// constant-quality mode at all, which is a warning rather than a failure.
    quality: &'static [Knob],
    /// Why the encoder has no constant-quality mode, when it has none.
    no_quality: Option<&'static str>,
}

const fn mapping(
    element: &'static str,
    bitrate: &'static [Knob],
    quality: &'static [Knob],
    no_quality: Option<&'static str>,
) -> Mapping {
    Mapping {
        element,
        bitrate,
        quality,
        no_quality,
    }
}

/// The H.264/H.265 quantiser range: the scale the presets are written on.
const QP_51: u32 = 51;

/// The AV1 quantiser range of the software encoders (aom and SVT-AV1).
const QP_63: u32 = 63;

/// The AV1 quantiser range of the hardware encoders and rav1e.
const QP_255: u32 = 255;

/// NVENC: `bitrate` is in kbit/s and `rc-mode` selects the mode; the constant
/// quantiser lives on `qp-const` on the nvcodec elements and on the per-frame
/// `qp-const-i` on the newer device-specific ones.
const NVENC_BITRATE: &[Knob] = &[
    knob(&["rc-mode"], Setting::Mode(&["cbr", "cbr-ld-hq", "vbr"])),
    knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Kbps)),
];

/// VA-API: `rate-control` selects the mode and the quantiser is per frame
/// type, so I, P and B frames are all pinned to the same value; `vaav1enc`
/// carries a single `qp` instead.
const VA_BITRATE: &[Knob] = &[
    knob(&["rate-control"], Setting::Mode(&["cbr", "vbr"])),
    knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Kbps)),
];

/// AMF: `rate-control` selects the mode, `qp-i` and `qp-p` carry the constant
/// quantiser.
const AMF_BITRATE: &[Knob] = &[
    knob(&["rate-control"], Setting::Mode(&["cbr", "vbr-latency"])),
    knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Kbps)),
];

/// The mapping for every encoder in [`crate::encoder`]'s catalogue.
///
/// The units are the ones each element's own documentation states: kbit/s for
/// everything except `rav1enc`, which takes bits per second.
const MAPPINGS: &[Mapping] = &[
    // NVIDIA NVENC.
    mapping(
        "nvh264enc",
        NVENC_BITRATE,
        &[
            knob(&["rc-mode"], Setting::Mode(&["constqp", "cqp"])),
            knob(
                &["qp-const", "qp-const-i", "qp-i"],
                Setting::Quantiser { max: QP_51 },
            ),
        ],
        None,
    ),
    mapping(
        "nvh265enc",
        NVENC_BITRATE,
        &[
            knob(&["rc-mode"], Setting::Mode(&["constqp", "cqp"])),
            knob(
                &["qp-const", "qp-const-i", "qp-i"],
                Setting::Quantiser { max: QP_51 },
            ),
        ],
        None,
    ),
    mapping(
        "nvd3d11h264enc",
        NVENC_BITRATE,
        &[
            knob(&["rc-mode"], Setting::Mode(&["constqp", "cqp"])),
            knob(
                &["qp-const", "qp-const-i", "qp-i"],
                Setting::Quantiser { max: QP_51 },
            ),
        ],
        None,
    ),
    mapping(
        "nvd3d11h265enc",
        NVENC_BITRATE,
        &[
            knob(&["rc-mode"], Setting::Mode(&["constqp", "cqp"])),
            knob(
                &["qp-const", "qp-const-i", "qp-i"],
                Setting::Quantiser { max: QP_51 },
            ),
        ],
        None,
    ),
    mapping(
        "nvautogpuh264enc",
        NVENC_BITRATE,
        &[
            knob(&["rc-mode"], Setting::Mode(&["constqp", "cqp"])),
            knob(
                &["qp-const", "qp-const-i", "qp-i"],
                Setting::Quantiser { max: QP_51 },
            ),
        ],
        None,
    ),
    mapping(
        "nvautogpuh265enc",
        NVENC_BITRATE,
        &[
            knob(&["rc-mode"], Setting::Mode(&["constqp", "cqp"])),
            knob(
                &["qp-const", "qp-const-i", "qp-i"],
                Setting::Quantiser { max: QP_51 },
            ),
        ],
        None,
    ),
    mapping(
        "nvav1enc",
        NVENC_BITRATE,
        &[
            knob(&["rc-mode"], Setting::Mode(&["constqp", "cqp"])),
            knob(
                &["qp-const", "qp-const-i", "qp-i"],
                Setting::Quantiser { max: QP_255 },
            ),
        ],
        None,
    ),
    // VA-API.
    mapping(
        "vah264enc",
        VA_BITRATE,
        &[
            knob(&["rate-control"], Setting::Mode(&["cqp"])),
            knob(&["qpi", "qp"], Setting::Quantiser { max: QP_51 }),
            knob(&["qpp", "qp"], Setting::Quantiser { max: QP_51 }),
            knob(&["qpb", "qp"], Setting::Quantiser { max: QP_51 }),
        ],
        None,
    ),
    mapping(
        "vah265enc",
        VA_BITRATE,
        &[
            knob(&["rate-control"], Setting::Mode(&["cqp"])),
            knob(&["qpi", "qp"], Setting::Quantiser { max: QP_51 }),
            knob(&["qpp", "qp"], Setting::Quantiser { max: QP_51 }),
            knob(&["qpb", "qp"], Setting::Quantiser { max: QP_51 }),
        ],
        None,
    ),
    mapping(
        "vaav1enc",
        VA_BITRATE,
        &[
            knob(&["rate-control"], Setting::Mode(&["cqp"])),
            knob(&["qp", "qpi"], Setting::Quantiser { max: QP_255 }),
        ],
        None,
    ),
    // AMD AMF.
    mapping(
        "amfh264enc",
        AMF_BITRATE,
        &[
            knob(&["rate-control"], Setting::Mode(&["cqp"])),
            knob(&["qp-i", "qp"], Setting::Quantiser { max: QP_51 }),
            knob(&["qp-p", "qp"], Setting::Quantiser { max: QP_51 }),
        ],
        None,
    ),
    mapping(
        "amfh265enc",
        AMF_BITRATE,
        &[
            knob(&["rate-control"], Setting::Mode(&["cqp"])),
            knob(&["qp-i", "qp"], Setting::Quantiser { max: QP_51 }),
            knob(&["qp-p", "qp"], Setting::Quantiser { max: QP_51 }),
        ],
        None,
    ),
    mapping(
        "amfav1enc",
        AMF_BITRATE,
        &[
            knob(&["rate-control"], Setting::Mode(&["cqp"])),
            knob(&["qp-i", "qp"], Setting::Quantiser { max: QP_255 }),
            knob(&["qp-p", "qp"], Setting::Quantiser { max: QP_255 }),
        ],
        None,
    ),
    // Apple VideoToolbox: bitrate only. The element has no constant-quality
    // mode for H.264 or HEVC — `quality` is a ProRes knob — so a CRF preset
    // warns and the encoder keeps its default.
    mapping(
        "vtenc_h264",
        &[knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Kbps))],
        &[],
        Some("VideoToolbox encodes H.264 at an average bitrate and has no constant-quality mode"),
    ),
    mapping(
        "vtenc_h265",
        &[knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Kbps))],
        &[],
        Some("VideoToolbox encodes HEVC at an average bitrate and has no constant-quality mode"),
    ),
    // Windows Media Foundation: bitrate only; its quality-vs-speed knob is
    // not a quantiser.
    mapping(
        "mfh264enc",
        &[knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Kbps))],
        &[],
        Some(
            "Media Foundation encodes H.264 at an average bitrate and has no constant-quality mode",
        ),
    ),
    mapping(
        "mfh265enc",
        &[knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Kbps))],
        &[],
        Some(
            "Media Foundation encodes HEVC at an average bitrate and has no constant-quality mode",
        ),
    ),
    // Software. `x264enc` distinguishes constant quantiser (`quant`) from
    // constant quality (`qual`), and CRF is the latter.
    mapping(
        "x264enc",
        &[
            knob(&["pass"], Setting::Mode(&["cbr"])),
            knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Kbps)),
        ],
        &[
            knob(&["pass"], Setting::Mode(&["qual", "quant"])),
            knob(&["quantizer"], Setting::Quantiser { max: QP_51 }),
        ],
        None,
    ),
    // `x265enc` has no mode enum: setting `qp` is what puts it in CQP mode.
    mapping(
        "x265enc",
        &[knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Kbps))],
        &[knob(&["qp"], Setting::Quantiser { max: QP_51 })],
        None,
    ),
    // SVT-AV1 takes a CRF directly, on the AV1 0..=63 scale.
    mapping(
        "svtav1enc",
        &[knob(
            &["target-bitrate", "bitrate"],
            Setting::Bitrate(BitrateUnit::Kbps),
        )],
        &[knob(&["crf", "cqp"], Setting::Quantiser { max: QP_63 })],
        None,
    ),
    // aom: `end-usage=q` is constant quality, pinned by the quantiser bounds.
    mapping(
        "av1enc",
        &[
            knob(&["end-usage"], Setting::Mode(&["vbr", "cbr"])),
            knob(
                &["target-bitrate", "bitrate"],
                Setting::Bitrate(BitrateUnit::Kbps),
            ),
        ],
        &[
            knob(&["end-usage"], Setting::Mode(&["q", "cq"])),
            knob(&["min-quantizer"], Setting::Quantiser { max: QP_63 }),
            knob(&["max-quantizer"], Setting::Quantiser { max: QP_63 }),
        ],
        None,
    ),
    // rav1e is the one video encoder whose bitrate is in bits per second.
    mapping(
        "rav1enc",
        &[knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Bps))],
        &[knob(&["quantizer"], Setting::Quantiser { max: QP_255 })],
        None,
    ),
];

/// The mapping for `element`, if it is catalogued.
fn mapping_for(element: &str) -> Option<&'static Mapping> {
    MAPPINGS.iter().find(|entry| entry.element == element)
}

/// True when `element` has a rate-control mapping.
///
/// The catalogue test in [`crate::encoder`] uses this to keep the two lists
/// from drifting apart.
#[must_use]
pub fn is_mapped(element: &str) -> bool {
    mapping_for(element).is_some()
}

/// Drives `encoder` at `quality`, returning what could not be applied.
///
/// Nothing here fails an export: an encoder the table does not know, a
/// property a plugin version does not carry and a codec with no
/// constant-quality mode all come back as warnings, and the element keeps
/// whatever rate control it defaults to.
#[must_use]
pub fn apply_video_quality(
    encoder: &gst::Element,
    element: &str,
    quality: VideoQuality,
) -> Vec<String> {
    let Some(mapping) = mapping_for(element) else {
        return vec![format!(
            "{element} has no known rate-control properties, so it encodes at its own default \
             instead of {quality}"
        )];
    };
    let knobs = match quality {
        VideoQuality::Bitrate { .. } => mapping.bitrate,
        VideoQuality::Crf { .. } => mapping.quality,
    };
    if knobs.is_empty() {
        let reason = mapping
            .no_quality
            .unwrap_or("the element has no equivalent property");
        return vec![format!(
            "{element} cannot be asked for {quality}: {reason}; it encodes at its own default"
        )];
    }
    let mut warnings = Vec::new();
    for knob in knobs {
        if let Err(warning) = apply_knob(encoder, element, knob, quality) {
            warnings.push(warning);
        }
    }
    warnings
}

/// Asks `encoder` for `kbps` kbit/s of audio, returning what could not be
/// applied.
///
/// Every audio encoder the exporter plugs takes its bitrate in bits per
/// second — `avenc_aac`, `voaacenc`, `fdkaacenc`, `faac` and `opusenc` all
/// document it that way — so the one knob covers them all. A lossless encoder
/// has no bitrate and is never given one: the preset carries `None`.
#[must_use]
pub fn apply_audio_bitrate(encoder: &gst::Element, element: &str, kbps: u32) -> Vec<String> {
    let knob = knob(&["bitrate"], Setting::Bitrate(BitrateUnit::Bps));
    match apply_knob(encoder, element, &knob, VideoQuality::Bitrate { kbps }) {
        Ok(()) => Vec::new(),
        Err(warning) => vec![warning],
    }
}

/// Sets one knob, or says why it could not be set.
fn apply_knob(
    encoder: &gst::Element,
    element: &str,
    knob: &Knob,
    quality: VideoQuality,
) -> Result<(), String> {
    for name in knob.names {
        let Some(spec) = encoder.find_property(name) else {
            continue;
        };
        return match knob.setting {
            Setting::Mode(nicks) => set_mode(encoder, element, name, &spec, nicks),
            Setting::Bitrate(unit) => {
                let VideoQuality::Bitrate { kbps } = quality else {
                    return Ok(());
                };
                set_number(encoder, element, name, &spec, unit.value(kbps))
            }
            Setting::Quantiser { max } => {
                let VideoQuality::Crf { value } = quality else {
                    return Ok(());
                };
                set_number(
                    encoder,
                    element,
                    name,
                    &spec,
                    u64::from(scale_crf(value, max)),
                )
            }
        };
    }
    Err(format!(
        "{element} carries none of the properties {names:?}, so {quality} is not fully applied",
        names = knob.names
    ))
}

/// Sets an enum property to the first nick its own enum carries.
fn set_mode(
    encoder: &gst::Element,
    element: &str,
    name: &str,
    spec: &glib::ParamSpec,
    nicks: &[&str],
) -> Result<(), String> {
    let Some(class) = glib::EnumClass::with_type(spec.value_type()) else {
        return Err(format!(
            "{element}'s '{name}' is not an enum property, so its rate-control mode is left alone"
        ));
    };
    for nick in nicks {
        if let Some(value) = class.to_value_by_nick(nick) {
            encoder.set_property_from_value(name, &value);
            return Ok(());
        }
    }
    Err(format!(
        "{element}'s '{name}' has none of the modes {nicks:?}, so its rate control is left alone"
    ))
}

/// Sets a numeric property, clamped to its own range.
fn set_number(
    encoder: &gst::Element,
    element: &str,
    name: &str,
    spec: &glib::ParamSpec,
    value: u64,
) -> Result<(), String> {
    if let Some(spec) = spec.downcast_ref::<glib::ParamSpecUInt>() {
        let clamped = clamp_u64(value, u64::from(spec.minimum()), u64::from(spec.maximum()));
        encoder.set_property(name, u32::try_from(clamped).unwrap_or(u32::MAX));
    } else if let Some(spec) = spec.downcast_ref::<glib::ParamSpecInt>() {
        let low = u64::try_from(spec.minimum()).unwrap_or(0);
        let high = u64::try_from(spec.maximum()).unwrap_or(0);
        let clamped = clamp_u64(value, low, high);
        encoder.set_property(name, i32::try_from(clamped).unwrap_or(i32::MAX));
    } else if let Some(spec) = spec.downcast_ref::<glib::ParamSpecUInt64>() {
        let clamped = clamp_u64(value, spec.minimum(), spec.maximum());
        encoder.set_property(name, clamped);
    } else if let Some(spec) = spec.downcast_ref::<glib::ParamSpecInt64>() {
        let low = u64::try_from(spec.minimum()).unwrap_or(0);
        let high = u64::try_from(spec.maximum()).unwrap_or(0);
        let clamped = clamp_u64(value, low, high);
        encoder.set_property(name, i64::try_from(clamped).unwrap_or(i64::MAX));
    } else {
        return Err(format!(
            "{element}'s '{name}' is not a number the exporter can set, so {value} is not applied"
        ));
    }
    Ok(())
}

/// `value` inside `low..=high`, with an inverted range treated as `low`.
fn clamp_u64(value: u64, low: u64, high: u64) -> u64 {
    if high < low {
        low
    } else {
        value.clamp(low, high)
    }
}

/// A CRF on the presets' 0..=51 scale, rescaled onto `0..=max`.
///
/// Integer arithmetic with round-to-nearest, so CRF 0 stays lossless and CRF
/// 51 stays the encoder's worst quality whatever its range.
#[must_use]
pub fn scale_crf(value: u8, max: u32) -> u32 {
    let value = u32::from(value.min(MAX_CRF));
    if max == QP_51 {
        return value;
    }
    (value * max + QP_51 / 2) / QP_51
}

#[cfg(test)]
mod tests {
    use super::{BitrateUnit, MAPPINGS, MAX_CRF, QP_63, QP_255, mapping_for, scale_crf};
    use crate::encoder::{CODECS, encoder_names};

    #[test]
    fn every_catalogued_encoder_has_a_mapping() {
        for codec in CODECS {
            for element in encoder_names(codec) {
                assert!(
                    mapping_for(element).is_some(),
                    "{element} has no rate-control mapping"
                );
            }
        }
    }

    #[test]
    fn every_mapping_names_a_catalogued_encoder() {
        let catalogued: Vec<&str> = CODECS.into_iter().flat_map(encoder_names).collect();
        for entry in MAPPINGS {
            assert!(
                catalogued.contains(&entry.element),
                "{} is mapped but not catalogued",
                entry.element
            );
        }
    }

    #[test]
    fn every_mapping_can_ask_for_a_bitrate() {
        for entry in MAPPINGS {
            assert!(
                !entry.bitrate.is_empty(),
                "{} has no bitrate knobs",
                entry.element
            );
        }
    }

    #[test]
    fn an_encoder_without_constant_quality_says_why() {
        for entry in MAPPINGS {
            assert_eq!(
                entry.quality.is_empty(),
                entry.no_quality.is_some(),
                "{} must explain a missing constant-quality mode exactly when it has none",
                entry.element
            );
        }
    }

    #[test]
    fn the_h264_scale_is_the_identity() {
        for value in 0..=MAX_CRF {
            assert_eq!(scale_crf(value, 51), u32::from(value));
        }
    }

    #[test]
    fn a_wider_quantiser_range_is_scaled_onto() {
        assert_eq!(scale_crf(0, QP_63), 0);
        assert_eq!(scale_crf(51, QP_63), QP_63);
        assert_eq!(scale_crf(20, QP_63), 25);
        assert_eq!(scale_crf(0, QP_255), 0);
        assert_eq!(scale_crf(51, QP_255), QP_255);
        assert_eq!(scale_crf(23, QP_255), 115);
    }

    #[test]
    fn a_crf_above_the_scale_is_clamped() {
        assert_eq!(scale_crf(200, QP_63), QP_63);
    }

    #[test]
    fn kilobits_and_bits_differ_by_a_thousand() {
        assert_eq!(BitrateUnit::Kbps.value(12_000), 12_000);
        assert_eq!(BitrateUnit::Bps.value(12_000), 12_000_000);
    }
}
