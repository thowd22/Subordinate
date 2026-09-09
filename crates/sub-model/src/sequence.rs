//! Sequences and the settings that define their canvas, timebase and colour.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_time::Rational;

use crate::codes;
use crate::ids::{MarkerId, SequenceId};
use crate::marker::Marker;
use crate::track::Track;

/// A pixel canvas size. Both dimensions are non-zero.
///
/// OTIO has no counterpart: it stores no rendering resolution.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(into = "ResolutionRepr", try_from = "ResolutionRepr")]
#[schemars(with = "ResolutionRepr")]
pub struct Resolution {
    width: u32,
    height: u32,
}

/// The serde form of a [`Resolution`], validated on the way in by
/// [`Resolution::new`] so a file can never carry a zero dimension.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ResolutionRepr {
    /// Width in pixels; never zero.
    width: u32,
    /// Height in pixels; never zero.
    height: u32,
}

impl From<Resolution> for ResolutionRepr {
    fn from(resolution: Resolution) -> Self {
        Self {
            width: resolution.width,
            height: resolution.height,
        }
    }
}

impl TryFrom<ResolutionRepr> for Resolution {
    type Error = SubError;

    fn try_from(repr: ResolutionRepr) -> SubResult<Self> {
        Self::new(repr.width, repr.height)
    }
}

impl Resolution {
    /// 1920x1080, the default sequence canvas.
    pub const HD_1080: Self = Self {
        width: 1920,
        height: 1080,
    };
    /// 3840x2160.
    pub const UHD_2160: Self = Self {
        width: 3840,
        height: 2160,
    };

    /// Creates a resolution.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_settings` if either dimension is zero.
    pub fn new(width: u32, height: u32) -> SubResult<Self> {
        if width == 0 || height == 0 {
            return Err(SubError::new(
                codes::INVALID_SETTINGS,
                "sequence resolution must be non-zero in both dimensions",
            )
            .with_detail("width", width)
            .with_detail("height", height));
        }
        Ok(Self { width, height })
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }
}

impl Default for Resolution {
    fn default() -> Self {
        Self::HD_1080
    }
}

/// The colour space a set of pixels is encoded in.
///
/// Per decision-3 the MVP stores these tags and renders assuming Rec.709; the
/// tags exist so adding colour management later is not a lossy migration.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpace {
    /// Rec. ITU-R BT.709, the MVP working space.
    #[default]
    Rec709,
    /// Rec. ITU-R BT.601 (standard definition).
    Rec601,
    /// Rec. ITU-R BT.2020.
    Rec2020,
    /// Not known, or not reported by the source.
    Unknown,
}

/// The transfer function (gamma curve) pixel values are encoded with.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TransferFunction {
    /// BT.709 / BT.1886 display gamma.
    #[default]
    Bt709,
    /// sRGB.
    Srgb,
    /// SMPTE ST 2084 perceptual quantizer (HDR10).
    Pq,
    /// Hybrid log-gamma.
    Hlg,
    /// Not known, or not reported by the source.
    Unknown,
}

/// The chromaticities of the red, green and blue primaries.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ColorPrimaries {
    /// BT.709 primaries.
    #[default]
    Bt709,
    /// BT.601 primaries.
    Bt601,
    /// BT.2020 primaries.
    Bt2020,
    /// DCI-P3.
    DciP3,
    /// Not known, or not reported by the source.
    Unknown,
}

/// The colour tags carried by a sequence or a media item (decision-3).
///
/// OTIO has no counterpart; this would live in `metadata` on an OTIO export.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct ColorTags {
    /// The encoding colour space.
    pub space: ColorSpace,
    /// The transfer function.
    pub transfer: TransferFunction,
    /// The primaries.
    pub primaries: ColorPrimaries,
}

impl ColorTags {
    /// The Rec.709 tags the MVP renderer assumes.
    pub const REC709: Self = Self {
        space: ColorSpace::Rec709,
        transfer: TransferFunction::Bt709,
        primaries: ColorPrimaries::Bt709,
    };
}

/// The canvas, timebase, audio rate and colour tags of a [`Sequence`].
///
/// OTIO stores only the rate, implicitly, on each item's `RationalTime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(into = "SequenceSettingsRepr", try_from = "SequenceSettingsRepr")]
#[schemars(with = "SequenceSettingsRepr")]
pub struct SequenceSettings {
    /// Render canvas size in pixels.
    pub resolution: Resolution,
    /// Editing timebase. Every timeline position in the sequence is expressed
    /// at this exact rational rate; never a float.
    pub frame_rate: Rational,
    /// Audio sample rate in hertz that the mixer renders at.
    pub sample_rate: u32,
    /// Colour tags (decision-3).
    pub color: ColorTags,
}

impl SequenceSettings {
    /// Creates settings, validating the audio sample rate.
    ///
    /// The frame rate needs no validation: [`Rational`] is already a strictly
    /// positive reduced fraction.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_settings` if `sample_rate` is zero.
    pub fn new(
        resolution: Resolution,
        frame_rate: Rational,
        sample_rate: u32,
        color: ColorTags,
    ) -> SubResult<Self> {
        if sample_rate == 0 {
            return Err(SubError::new(
                codes::INVALID_SETTINGS,
                "sequence audio sample rate must be non-zero",
            ));
        }
        Ok(Self {
            resolution,
            frame_rate,
            sample_rate,
            color,
        })
    }
}

/// The serde form of [`SequenceSettings`], validated on the way in by
/// [`SequenceSettings::new`] so a file can never carry a zero sample rate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SequenceSettingsRepr {
    /// Render canvas size in pixels.
    resolution: Resolution,
    /// Editing timebase.
    frame_rate: Rational,
    /// Audio sample rate in hertz; never zero.
    sample_rate: u32,
    /// Colour tags.
    color: ColorTags,
}

impl From<SequenceSettings> for SequenceSettingsRepr {
    fn from(settings: SequenceSettings) -> Self {
        Self {
            resolution: settings.resolution,
            frame_rate: settings.frame_rate,
            sample_rate: settings.sample_rate,
            color: settings.color,
        }
    }
}

impl TryFrom<SequenceSettingsRepr> for SequenceSettings {
    type Error = SubError;

    fn try_from(repr: SequenceSettingsRepr) -> SubResult<Self> {
        Self::new(
            repr.resolution,
            repr.frame_rate,
            repr.sample_rate,
            repr.color,
        )
    }
}

impl Default for SequenceSettings {
    /// 1920x1080 at 24 fps, 48 kHz audio, Rec.709.
    fn default() -> Self {
        Self {
            resolution: Resolution::HD_1080,
            frame_rate: Rational::FPS_24,
            sample_rate: 48_000,
            color: ColorTags::REC709,
        }
    }
}

/// One editable timeline: an ordered stack of [`Track`]s plus its settings.
///
/// OTIO counterpart: `Timeline`, whose single `tracks` child is a `Stack` of
/// `Track`s. Subordinate flattens that stack into [`Sequence::tracks`] because
/// nested stacks are out of MVP scope. Video tracks composite top-down, so
/// later entries in `tracks` render over earlier ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sequence {
    /// Stable identity, preserved across save, load and undo.
    pub id: SequenceId,
    /// Display name, shown on the sequence tab.
    pub name: String,
    /// Canvas, timebase, audio rate and colour tags.
    pub settings: SequenceSettings,
    /// Tracks, bottom-most first.
    pub tracks: Vec<Track>,
    /// Timeline-wide markers (OTIO `Timeline.tracks.markers`).
    pub markers: Vec<Marker>,
}

impl Sequence {
    /// Creates an empty sequence with a fresh identifier.
    #[must_use]
    pub fn new(name: impl Into<String>, settings: SequenceSettings) -> Self {
        Self {
            id: SequenceId::new(),
            name: name.into(),
            settings,
            tracks: Vec::new(),
            markers: Vec::new(),
        }
    }

    /// The track with `id`, if this sequence holds it.
    #[must_use]
    pub fn track(&self, id: crate::ids::TrackId) -> Option<&Track> {
        self.tracks.iter().find(|track| track.id == id)
    }

    /// The marker with `id`, if this sequence holds it.
    #[must_use]
    pub fn marker(&self, id: MarkerId) -> Option<&Marker> {
        self.markers.iter().find(|marker| marker.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_rejects_zero_dimensions() {
        assert_eq!(Resolution::new(1920, 1080).unwrap(), Resolution::HD_1080);
        for (w, h) in [(0, 1080), (1920, 0), (0, 0)] {
            let err = Resolution::new(w, h).unwrap_err();
            assert_eq!(err.code, codes::INVALID_SETTINGS);
        }
    }

    #[test]
    fn default_settings_are_hd_24fps_48khz_rec709() {
        let settings = SequenceSettings::default();
        assert_eq!(settings.resolution.width(), 1920);
        assert_eq!(settings.resolution.height(), 1080);
        assert_eq!(settings.frame_rate, Rational::FPS_24);
        assert_eq!(settings.sample_rate, 48_000);
        assert_eq!(settings.color, ColorTags::REC709);
    }

    #[test]
    fn settings_reject_a_zero_sample_rate() {
        let err = SequenceSettings::new(
            Resolution::UHD_2160,
            Rational::FPS_23_976,
            0,
            ColorTags::REC709,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::INVALID_SETTINGS);
    }

    #[test]
    fn settings_keep_broadcast_rates_exact() {
        let settings = SequenceSettings::new(
            Resolution::UHD_2160,
            Rational::FPS_29_97,
            48_000,
            ColorTags::REC709,
        )
        .unwrap();
        assert_eq!(settings.frame_rate.numerator(), 30_000);
        assert_eq!(settings.frame_rate.denominator(), 1001);
    }

    #[test]
    fn sequences_look_up_their_own_tracks() {
        use crate::track::{Track, TrackKind};

        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        let track = Track::new("V1", TrackKind::Video);
        let id = track.id;
        sequence.tracks.push(track);
        assert_eq!(sequence.track(id).map(|t| t.name.as_str()), Some("V1"));
        assert!(sequence.track(crate::ids::TrackId::new()).is_none());
    }
}
