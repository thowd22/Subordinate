//! Tracks and the items laid out along them: clips, gaps and transitions.

use sub_time::{RationalTime, TimeRange};

use crate::ids::{ClipId, MediaId, TrackId};
use crate::marker::Marker;

/// What a track carries.
///
/// OTIO counterpart: `Track.kind`, the `"Video"` / `"Audio"` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
/// Per-clip parameters (opacity, transform, gain, fades) are added by
/// TASK-3.2; they are deliberately not part of this type yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    /// Stable identity, preserved across save, load and undo.
    pub id: ClipId,
    /// Display name, shown on the timeline rectangle.
    pub name: String,
    /// The media this clip plays.
    pub media: MediaId,
    /// The portion of the source used, in source time.
    pub source_range: TimeRange,
    /// Markers anchored to this clip (OTIO `Clip.markers`).
    pub markers: Vec<Marker>,
}

impl Clip {
    /// Creates a clip with a fresh identifier.
    #[must_use]
    pub fn new(name: impl Into<String>, media: MediaId, source_range: TimeRange) -> Self {
        Self {
            id: ClipId::new(),
            name: name.into(),
            media,
            source_range,
            markers: Vec::new(),
        }
    }

    /// How long this clip occupies its track.
    #[must_use]
    pub fn duration(&self) -> RationalTime {
        self.source_range.duration()
    }
}

/// Empty time on a track: black picture or silence.
///
/// OTIO counterpart: `Gap`, whose `source_range` carries the duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// Stable identity, preserved across save, load and undo.
    pub id: TrackId,
    /// Display name, shown in the track header ("V1", "Dialogue").
    pub name: String,
    /// Whether this lane carries picture or sound.
    pub kind: TrackKind,
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

    /// The total track time occupied by the items, summed exactly at their
    /// common rate. `rate` is the sequence timebase, used as the rate of the
    /// zero an empty track returns.
    #[must_use]
    pub fn duration(&self, rate: sub_time::Rational) -> RationalTime {
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
    fn a_clip_reports_the_duration_of_its_source_range() {
        let clip = Clip::new("shot 1", MediaId::new(), range(48, 24));
        assert_eq!(clip.duration(), RationalTime::new(24, Rational::FPS_24));
        assert!(clip.markers.is_empty());
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
