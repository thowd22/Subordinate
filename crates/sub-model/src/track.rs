//! Tracks and the items laid out along them: clips, gaps and transitions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_time::{Rational, RationalTime, TimeRange};

use crate::codes;
use crate::ids::{ClipId, MediaId, TrackId};
use crate::marker::Marker;
use crate::params::{GainDb, Opacity, Transform};

/// What a track carries.
///
/// OTIO counterpart: `Track.kind`, the `"Video"` / `"Audio"` string.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Picture. Video tracks composite top-down.
    Video,
    /// Sound. Audio tracks sum in the mixer.
    Audio,
}

impl TrackKind {
    /// The OTIO `Track.kind` string this maps to.
    #[must_use]
    pub const fn as_otio_kind(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Audio => "Audio",
        }
    }
}

/// A reference to a slice of a [`MediaItem`](crate::MediaItem) placed on a
/// track.
///
/// OTIO counterpart: `Clip`, whose `media_reference` is an
/// `ExternalReference` and whose `source_range` selects the used portion of it.
/// Placement on the track is positional, exactly as in OTIO: a clip starts
/// where the preceding items end.
///
/// The fixed set of per-clip parameters the MVP supports (docs/PLAN.md §5.1)
/// live on the clip itself: [`Clip::opacity`], [`Clip::transform`],
/// [`Clip::gain`] and the two fades. Their invariants are checked by
/// [`Clip::validate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    /// Stable identity, preserved across save, load and undo.
    pub id: ClipId,
    /// Display name, shown on the timeline rectangle.
    pub name: String,
    /// The media this clip plays.
    pub media: MediaId,
    /// The portion of the source used, in source time.
    pub source_range: TimeRange,
    /// How opaque the picture is when composited. Fully opaque by default.
    pub opacity: Opacity,
    /// Position, scale and rotation on the sequence canvas. Identity by
    /// default.
    pub transform: Transform,
    /// Clip audio level in decibels. Unity by default.
    pub gain: GainDb,
    /// How long the clip ramps up from nothing at its head. Zero by default.
    pub fade_in: RationalTime,
    /// How long the clip ramps down to nothing at its tail. Zero by default.
    pub fade_out: RationalTime,
    /// Markers anchored to this clip (OTIO `Clip.markers`).
    pub markers: Vec<Marker>,
}

impl Clip {
    /// Creates a clip with a fresh identifier and default parameters: fully
    /// opaque, untransformed, unity gain and no fades.
    #[must_use]
    pub fn new(name: impl Into<String>, media: MediaId, source_range: TimeRange) -> Self {
        let zero = RationalTime::zero(source_range.duration().rate());
        Self {
            id: ClipId::new(),
            name: name.into(),
            media,
            source_range,
            opacity: Opacity::OPAQUE,
            transform: Transform::IDENTITY,
            gain: GainDb::UNITY,
            fade_in: zero,
            fade_out: zero,
            markers: Vec::new(),
        }
    }

    /// How long this clip occupies its track.
    #[must_use]
    pub fn duration(&self) -> RationalTime {
        self.source_range.duration()
    }

    /// The span this clip occupies in sequence time when it is placed at
    /// `start`.
    ///
    /// Placement is positional, exactly as in OTIO: the start comes from the
    /// items before it on the track, which is why it is a parameter here
    /// rather than a stored field. [`Track::timeline_range_of`] computes it
    /// for a clip on a track.
    ///
    /// Returns `None` only if `start + duration` is not exactly representable.
    #[must_use]
    pub fn timeline_range(&self, start: RationalTime) -> Option<TimeRange> {
        TimeRange::new(start, self.duration())
    }

    /// Checks the clip's invariants.
    ///
    /// The constructors of [`TimeRange`] and of the parameter types already
    /// reject most invalid values; this catches what only the whole clip can
    /// see, and re-checks the rest so that a clip rebuilt field by field (by a
    /// deserializer, a migration or a plugin) is still verified:
    ///
    /// - the source range lasts a non-negative time,
    /// - neither fade lasts a negative time,
    /// - the two fades together do not exceed the clip length.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_clip` describing the first broken invariant.
    pub fn validate(&self) -> SubResult<()> {
        let duration = self.duration();
        if duration.is_negative() {
            return Err(self
                .invalid("clip source range must last a non-negative time")
                .with_detail("duration", duration.to_string()));
        }

        for (field, fade) in [("fade_in", self.fade_in), ("fade_out", self.fade_out)] {
            if fade.is_negative() {
                return Err(self
                    .invalid("clip fade must last a non-negative time")
                    .with_detail("field", field)
                    .with_detail("fade", fade.to_string()));
            }
        }

        let fades = self.fade_in.checked_add(self.fade_out).ok_or_else(|| {
            self.invalid("clip fade durations cannot be added exactly")
                .with_detail("fade_in", self.fade_in.to_string())
                .with_detail("fade_out", self.fade_out.to_string())
        })?;
        if fades > duration {
            return Err(self
                .invalid("clip fades together may not exceed the clip length")
                .with_detail("fade_in", self.fade_in.to_string())
                .with_detail("fade_out", self.fade_out.to_string())
                .with_detail("duration", duration.to_string()));
        }
        Ok(())
    }

    /// A `model.invalid_clip` error naming this clip.
    fn invalid(&self, message: &str) -> SubError {
        SubError::new(codes::INVALID_CLIP, message)
            .with_detail("clip_id", self.id)
            .with_detail("clip_name", self.name.clone())
    }
}

/// Empty time on a track: black picture or silence.
///
/// OTIO counterpart: `Gap`, whose `source_range` carries the duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Gap {
    /// How long the gap lasts.
    pub duration: RationalTime,
}

impl Gap {
    /// Creates a gap of `duration`.
    #[must_use]
    pub const fn new(duration: RationalTime) -> Self {
        Self { duration }
    }
}

/// A blend across the cut between the two items either side of it.
///
/// OTIO counterpart: `Transition`. `Crossfade` maps to
/// `transition_type: "SMPTE_Dissolve"`; the offsets are OTIO's `in_offset` and
/// `out_offset`, the amounts the transition reaches back into the outgoing
/// item and forward into the incoming one. Crossfade is the only transition in
/// the MVP (docs/PLAN.md §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    /// A linear dissolve centred on the cut.
    Crossfade {
        /// How far the transition reaches back into the outgoing item.
        in_offset: RationalTime,
        /// How far it reaches forward into the incoming item.
        out_offset: RationalTime,
    },
}

impl Transition {
    /// Creates a crossfade from its two offsets.
    #[must_use]
    pub const fn crossfade(in_offset: RationalTime, out_offset: RationalTime) -> Self {
        Self::Crossfade {
            in_offset,
            out_offset,
        }
    }

    /// How far the blend reaches back into the outgoing item.
    #[must_use]
    pub const fn in_offset(&self) -> RationalTime {
        match self {
            Self::Crossfade { in_offset, .. } => *in_offset,
        }
    }

    /// How far the blend reaches forward into the incoming item.
    #[must_use]
    pub const fn out_offset(&self) -> RationalTime {
        match self {
            Self::Crossfade { out_offset, .. } => *out_offset,
        }
    }

    /// The total length of the blend.
    #[must_use]
    pub fn duration(&self) -> RationalTime {
        match self {
            Self::Crossfade {
                in_offset,
                out_offset,
            } => *in_offset + *out_offset,
        }
    }
}

/// One entry in a track's ordered child list.
///
/// OTIO counterpart: the `Composable` children of a `Track`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackItem {
    /// A piece of media.
    Clip(Clip),
    /// Empty time.
    Gap(Gap),
    /// A blend across the neighbouring cut. Unlike clips and gaps, a
    /// transition consumes no track time of its own; it overlaps its
    /// neighbours.
    Transition(Transition),
}

impl TrackItem {
    /// The clip, if this item is one.
    #[must_use]
    pub const fn as_clip(&self) -> Option<&Clip> {
        match self {
            Self::Clip(clip) => Some(clip),
            _ => None,
        }
    }

    /// The gap, if this item is one.
    #[must_use]
    pub const fn as_gap(&self) -> Option<&Gap> {
        match self {
            Self::Gap(gap) => Some(gap),
            _ => None,
        }
    }

    /// The transition, if this item is one.
    #[must_use]
    pub const fn as_transition(&self) -> Option<&Transition> {
        match self {
            Self::Transition(transition) => Some(transition),
            _ => None,
        }
    }

    /// How much track time this item occupies. Transitions occupy none.
    #[must_use]
    pub fn track_duration(&self, rate: sub_time::Rational) -> RationalTime {
        match self {
            Self::Clip(clip) => clip.duration(),
            Self::Gap(gap) => gap.duration,
            Self::Transition(_) => RationalTime::zero(rate),
        }
    }
}

impl From<Clip> for TrackItem {
    fn from(clip: Clip) -> Self {
        Self::Clip(clip)
    }
}

impl From<Gap> for TrackItem {
    fn from(gap: Gap) -> Self {
        Self::Gap(gap)
    }
}

impl From<Transition> for TrackItem {
    fn from(transition: Transition) -> Self {
        Self::Transition(transition)
    }
}

/// One lane of a sequence: an ordered list of clips, gaps and transitions.
///
/// OTIO counterpart: `Track`, a `Composition` whose children are laid end to
/// end in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Track {
    /// Stable identity, preserved across save, load and undo.
    pub id: TrackId,
    /// Display name, shown in the track header ("V1", "Dialogue").
    pub name: String,
    /// Whether this lane carries picture or sound.
    pub kind: TrackKind,
    /// Whether the lane is silenced: a muted audio track contributes nothing
    /// to the mixer and a muted video track nothing to the composite.
    ///
    /// Defaults to false, and is defaulted on the way in so a project file
    /// written before the flag existed still loads.
    #[serde(default)]
    pub muted: bool,
    /// Whether the lane is locked against edits.
    ///
    /// Clip commands refuse to touch a locked track (`edit.track_locked`);
    /// track header commands, including the one that unlocks it, still apply.
    #[serde(default)]
    pub locked: bool,
    /// The items, in playback order.
    pub items: Vec<TrackItem>,
}

impl Track {
    /// Creates an empty track with a fresh identifier.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: TrackKind) -> Self {
        Self {
            id: TrackId::new(),
            name: name.into(),
            kind,
            muted: false,
            locked: false,
            items: Vec::new(),
        }
    }

    /// Every clip on this track, in order.
    pub fn clips(&self) -> impl Iterator<Item = &Clip> {
        self.items.iter().filter_map(TrackItem::as_clip)
    }

    /// The clip with `id`, if this track holds it.
    #[must_use]
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips().find(|clip| clip.id == id)
    }

    /// Every item paired with the span it occupies in sequence time.
    ///
    /// Placement is positional: each item starts where the previous ones end,
    /// and a transition, occupying no track time of its own, yields an empty
    /// range at the cut. `rate` is the sequence timebase, used as the rate of
    /// the zero the walk starts from.
    ///
    /// Iteration stops early if a duration is negative or a running total is
    /// not exactly representable, neither of which the model's constructors
    /// allow.
    pub fn placements(&self, rate: Rational) -> impl Iterator<Item = (&TrackItem, TimeRange)> {
        let mut cursor = RationalTime::zero(rate);
        self.items.iter().map_while(move |item| {
            let range = TimeRange::new(cursor, item.track_duration(rate))?;
            cursor = range.end_exclusive();
            Some((item, range))
        })
    }

    /// Every clip paired with the span it occupies in sequence time.
    pub fn clip_placements(&self, rate: Rational) -> impl Iterator<Item = (&Clip, TimeRange)> {
        self.placements(rate)
            .filter_map(|(item, range)| item.as_clip().map(|clip| (clip, range)))
    }

    /// The span the clip with `id` occupies in sequence time, if this track
    /// holds it.
    #[must_use]
    pub fn timeline_range_of(&self, id: ClipId, rate: Rational) -> Option<TimeRange> {
        self.clip_placements(rate)
            .find(|(clip, _)| clip.id == id)
            .map(|(_, range)| range)
    }

    /// The total track time occupied by the items, summed exactly at their
    /// common rate. `rate` is the sequence timebase, used as the rate of the
    /// zero an empty track returns.
    #[must_use]
    pub fn duration(&self, rate: Rational) -> RationalTime {
        self.items
            .iter()
            .fold(RationalTime::zero(rate), |total, item| {
                total + item.track_duration(rate)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_time::Rational;

    fn range(start: i64, len: i64) -> TimeRange {
        TimeRange::new(
            RationalTime::new(start, Rational::FPS_24),
            RationalTime::new(len, Rational::FPS_24),
        )
        .unwrap()
    }

    #[test]
    fn track_kinds_map_to_otio_strings() {
        assert_eq!(TrackKind::Video.as_otio_kind(), "Video");
        assert_eq!(TrackKind::Audio.as_otio_kind(), "Audio");
    }

    #[test]
    fn a_new_track_is_neither_muted_nor_locked() {
        let track = Track::new("V1", TrackKind::Video);
        assert!(!track.muted);
        assert!(!track.locked);
    }

    #[test]
    fn a_track_written_before_the_flags_existed_still_loads() {
        let id = TrackId::new();
        let text = serde_json::json!({
            "id": id.to_string(),
            "name": "V1",
            "kind": "video",
            "items": [],
        })
        .to_string();

        let track: Track = serde_json::from_str(&text).unwrap();
        assert_eq!(track.id, id);
        assert!(!track.muted);
        assert!(!track.locked);
    }

    #[test]
    fn a_clip_reports_the_duration_of_its_source_range() {
        let clip = Clip::new("shot 1", MediaId::new(), range(48, 24));
        assert_eq!(clip.duration(), RationalTime::new(24, Rational::FPS_24));
        assert!(clip.markers.is_empty());
    }

    #[test]
    fn a_new_clip_has_neutral_parameters_and_validates() {
        let clip = Clip::new("shot 1", MediaId::new(), range(48, 24));
        assert_eq!(clip.opacity, Opacity::OPAQUE);
        assert_eq!(clip.transform, Transform::IDENTITY);
        assert_eq!(clip.gain, GainDb::UNITY);
        assert!(clip.fade_in.is_zero());
        assert!(clip.fade_out.is_zero());
        assert!(clip.validate().is_ok());
    }

    #[test]
    fn a_clip_carries_every_mvp_parameter() {
        use crate::params::{Fixed6, Point2, Scale2};

        let mut clip = Clip::new("shot 1", MediaId::new(), range(0, 48));
        clip.opacity = Opacity::from_f64(0.5).unwrap();
        clip.transform = Transform::new(
            Point2::new(Fixed6::from_units(-16), Fixed6::from_units(8)),
            Scale2::uniform(Fixed6::from_micros(1_500_000)).unwrap(),
            Fixed6::from_units(45),
        );
        clip.gain = GainDb::from_f64(-6.0).unwrap();
        clip.fade_in = RationalTime::new(12, Rational::FPS_24);
        clip.fade_out = RationalTime::new(24, Rational::FPS_24);

        assert!(clip.validate().is_ok());
        assert!((clip.opacity.as_f32() - 0.5).abs() < f32::EPSILON);
        assert!((clip.gain.as_f32() + 6.0).abs() < f32::EPSILON);
        assert_eq!(clip.transform.scale.x().micros(), 1_500_000);
    }

    #[test]
    fn fades_may_exactly_fill_the_clip_but_not_exceed_it() {
        let half = RationalTime::new(24, Rational::FPS_24);
        let mut clip = Clip::new("shot 1", MediaId::new(), range(0, 48));
        clip.fade_in = half;
        clip.fade_out = half;
        assert!(clip.validate().is_ok());

        clip.fade_out = RationalTime::new(25, Rational::FPS_24);
        let err = clip.validate().unwrap_err();
        assert_eq!(err.code, codes::INVALID_CLIP);
        assert_eq!(err.details["duration"], "48@24");
        assert_eq!(
            err.details["clip_id"],
            serde_json::json!(clip.id.to_string())
        );
    }

    #[test]
    fn fades_are_compared_exactly_across_rates() {
        // One second of fade at 25 fps against a two-second clip at 24 fps.
        let mut clip = Clip::new("shot 1", MediaId::new(), range(0, 48));
        clip.fade_in = RationalTime::new(25, Rational::FPS_25);
        clip.fade_out = RationalTime::new(24, Rational::FPS_24);
        assert!(clip.validate().is_ok());

        clip.fade_out = RationalTime::new(25, Rational::FPS_24);
        assert_eq!(clip.validate().unwrap_err().code, codes::INVALID_CLIP);
    }

    #[test]
    fn a_negative_fade_is_rejected() {
        for field in ["fade_in", "fade_out"] {
            let mut clip = Clip::new("shot 1", MediaId::new(), range(0, 48));
            let negative = RationalTime::new(-1, Rational::FPS_24);
            if field == "fade_in" {
                clip.fade_in = negative;
            } else {
                clip.fade_out = negative;
            }
            let err = clip.validate().unwrap_err();
            assert_eq!(err.code, codes::INVALID_CLIP);
            assert_eq!(err.details["field"], field);
        }
    }

    #[test]
    fn clips_report_their_placement_in_sequence_time() {
        let rate = Rational::FPS_24;
        let mut track = Track::new("V1", TrackKind::Video);
        let first = Clip::new("a", MediaId::new(), range(0, 24));
        let first_id = first.id;
        let second = Clip::new("b", MediaId::new(), range(100, 12));
        let second_id = second.id;
        track.items.push(first.into());
        track
            .items
            .push(Gap::new(RationalTime::new(6, rate)).into());
        track.items.push(second.into());

        let placements: Vec<_> = track.clip_placements(rate).collect();
        assert_eq!(placements.len(), 2);
        assert_eq!(placements[0].1.start(), RationalTime::zero(rate));
        assert_eq!(placements[1].1.start(), RationalTime::new(30, rate));
        assert_eq!(placements[1].1.duration(), RationalTime::new(12, rate));

        assert_eq!(
            track.timeline_range_of(second_id, rate),
            Some(TimeRange::new(RationalTime::new(30, rate), RationalTime::new(12, rate)).unwrap())
        );
        assert_eq!(
            track
                .timeline_range_of(first_id, rate)
                .map(TimeRange::start),
            Some(RationalTime::zero(rate))
        );
        assert!(track.timeline_range_of(ClipId::new(), rate).is_none());
    }

    #[test]
    fn transitions_are_placed_as_empty_ranges_at_the_cut() {
        let rate = Rational::FPS_24;
        let half = RationalTime::new(6, rate);
        let mut track = Track::new("V1", TrackKind::Video);
        track
            .items
            .push(Clip::new("a", MediaId::new(), range(0, 24)).into());
        track.items.push(Transition::crossfade(half, half).into());
        track
            .items
            .push(Clip::new("b", MediaId::new(), range(0, 24)).into());

        let placements: Vec<_> = track.placements(rate).collect();
        assert_eq!(placements.len(), 3);
        assert!(placements[1].1.is_empty());
        assert_eq!(placements[1].1.start(), RationalTime::new(24, rate));
        assert_eq!(placements[2].1.start(), RationalTime::new(24, rate));
        assert_eq!(placements[2].1.end_exclusive(), track.duration(rate));
    }

    #[test]
    fn a_clip_placed_alone_spans_its_own_duration() {
        let rate = Rational::FPS_24;
        let clip = Clip::new("a", MediaId::new(), range(48, 24));
        let placed = clip.timeline_range(RationalTime::new(10, rate)).unwrap();
        assert_eq!(placed.start(), RationalTime::new(10, rate));
        assert_eq!(placed.duration(), RationalTime::new(24, rate));
        assert_eq!(placed.end_exclusive(), RationalTime::new(34, rate));
    }

    #[test]
    fn a_crossfade_duration_is_the_sum_of_its_offsets() {
        let half = RationalTime::new(6, Rational::FPS_24);
        let transition = Transition::crossfade(half, half);
        assert_eq!(
            transition.duration(),
            RationalTime::new(12, Rational::FPS_24)
        );
    }

    #[test]
    fn transitions_consume_no_track_time() {
        let rate = Rational::FPS_24;
        let half = RationalTime::new(6, rate);
        let mut track = Track::new("V1", TrackKind::Video);
        track
            .items
            .push(Clip::new("a", MediaId::new(), range(0, 24)).into());
        track.items.push(Transition::crossfade(half, half).into());
        track
            .items
            .push(Clip::new("b", MediaId::new(), range(0, 24)).into());
        track
            .items
            .push(Gap::new(RationalTime::new(12, rate)).into());

        assert_eq!(track.duration(rate), RationalTime::new(60, rate));
        assert_eq!(track.clips().count(), 2);
    }

    #[test]
    fn tracks_look_up_their_own_clips() {
        let mut track = Track::new("V1", TrackKind::Video);
        let clip = Clip::new("a", MediaId::new(), range(0, 10));
        let id = clip.id;
        track.items.push(clip.into());
        track
            .items
            .push(Gap::new(RationalTime::new(2, Rational::FPS_24)).into());
        assert_eq!(track.clip(id).map(|c| c.name.as_str()), Some("a"));
        assert!(track.clip(ClipId::new()).is_none());
        assert!(track.items[1].as_gap().is_some());
        assert!(track.items[1].as_clip().is_none());
        assert!(track.items[1].as_transition().is_none());
    }
}
