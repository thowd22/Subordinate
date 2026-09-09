//! Conversions between the WIT records and the crates they mirror.
//!
//! The WIT in `wit/subordinate-plugin.wit` is deliberately a narrow copy of the
//! model: `rational-time` is [`RationalTime`], `error` is [`SubError`], and
//! every identifier record wraps the same UUID text the project file uses. This
//! module is the one place those two vocabularies meet, so a host import
//! implementation (TASK-84) never hand-builds a record and never re-derives the
//! rules for what a valid one is.
//!
//! Conversions towards WIT are infallible: the model types already hold their
//! invariants. Conversions back are fallible, because a plugin is untrusted
//! input and may send a zero denominator or a string that is not a UUID; those
//! come back as a [`SubError`] with a `plugin.*` code.
//!
//! ```
//! use sub_plugin::WitRationalTime;
//! use sub_time::{Rational, RationalTime};
//!
//! let playhead = RationalTime::from_frames(48, Rational::FPS_23_976);
//! let crossed = WitRationalTime::from(playhead);
//! assert_eq!(crossed.rate.numerator, 24_000);
//! assert_eq!(crossed.rate.denominator, 1_001);
//! assert_eq!(RationalTime::try_from(crossed).unwrap(), playhead);
//! ```

use sub_core::{SubError, SubResult};
use sub_model::{
    ClipId, Marker, MarkerId, MediaId, Project, ProjectId, Resolution, Sequence, SequenceId, Track,
    TrackId, TrackKind,
};
use sub_time::{Rational, RationalTime, TimeRange};

use crate::bindings::subordinate::plugin::command_api as wit_api;
use crate::bindings::subordinate::plugin::types as wit_types;
use crate::codes;

impl From<&SubError> for wit_types::Error {
    /// Flattens a host error into the record a plugin sees.
    ///
    /// `code` and `message` cross verbatim and every detail crosses as its
    /// compact JSON text. [`SubError::cause`] is deliberately dropped: a cause
    /// chain is host-internal and may name paths a sandboxed plugin must not
    /// learn.
    fn from(error: &SubError) -> Self {
        Self {
            code: error.code.as_str().to_owned(),
            message: error.message.clone(),
            details: error
                .details
                .iter()
                .map(|(key, value)| wit_types::Detail {
                    key: key.clone(),
                    value: value.to_string(),
                })
                .collect(),
        }
    }
}

impl From<SubError> for wit_types::Error {
    fn from(error: SubError) -> Self {
        Self::from(&error)
    }
}

impl From<wit_types::Error> for SubError {
    /// Lifts a plugin's error back into a [`SubError`].
    ///
    /// A plugin may send anything, so a code that is not a valid
    /// [`ErrorCode`](sub_core::ErrorCode) is replaced by
    /// `plugin.invalid_error_code` with the original kept as a detail rather
    /// than discarded. A detail whose value is not JSON crosses back as a JSON
    /// string, so no information is lost either way.
    fn from(error: wit_types::Error) -> Self {
        let mut lifted = match sub_core::ErrorCode::parse(error.code.clone()) {
            Ok(code) => Self::new(code, error.message),
            Err(_) => Self::new(codes::INVALID_ERROR_CODE, error.message)
                .with_detail("plugin_code", error.code),
        };
        for wit_types::Detail { key, value } in error.details {
            let parsed = match serde_json::from_str::<serde_json::Value>(&value) {
                Ok(parsed) => parsed,
                Err(_) => serde_json::Value::String(value),
            };
            lifted = lifted.with_detail(key, parsed);
        }
        lifted
    }
}

impl From<Rational> for wit_types::Rational {
    fn from(rate: Rational) -> Self {
        Self {
            numerator: rate.numerator(),
            denominator: rate.denominator(),
        }
    }
}

impl TryFrom<wit_types::Rational> for Rational {
    type Error = SubError;

    /// # Errors
    ///
    /// Returns `plugin.invalid_rational` if either term is zero.
    fn try_from(rate: wit_types::Rational) -> SubResult<Self> {
        Self::new(rate.numerator, rate.denominator).ok_or_else(|| {
            SubError::new(
                codes::INVALID_RATIONAL,
                "a rate must have a non-zero numerator and denominator",
            )
            .with_detail("numerator", rate.numerator)
            .with_detail("denominator", rate.denominator)
        })
    }
}

impl From<RationalTime> for wit_types::RationalTime {
    fn from(time: RationalTime) -> Self {
        Self {
            value: time.value(),
            rate: time.rate().into(),
        }
    }
}

impl TryFrom<wit_types::RationalTime> for RationalTime {
    type Error = SubError;

    /// # Errors
    ///
    /// Returns `plugin.invalid_rational` if the rate is not a valid fraction.
    fn try_from(time: wit_types::RationalTime) -> SubResult<Self> {
        Ok(Self::new(time.value, Rational::try_from(time.rate)?))
    }
}

impl From<TimeRange> for wit_types::TimeRange {
    fn from(range: TimeRange) -> Self {
        Self {
            start: range.start().into(),
            duration: range.duration().into(),
        }
    }
}

impl TryFrom<wit_types::TimeRange> for TimeRange {
    type Error = SubError;

    /// # Errors
    ///
    /// Returns `plugin.invalid_rational` if either endpoint carries an invalid
    /// rate, or `plugin.invalid_time_range` if the duration is negative or the
    /// two endpoints are not at the same rate.
    fn try_from(range: wit_types::TimeRange) -> SubResult<Self> {
        let start = RationalTime::try_from(range.start)?;
        let duration = RationalTime::try_from(range.duration)?;
        Self::new(start, duration).ok_or_else(|| {
            SubError::new(
                codes::INVALID_TIME_RANGE,
                "a time range needs a non-negative duration at the start's rate",
            )
            .with_detail("start_rate", start.rate().to_string())
            .with_detail("duration_rate", duration.rate().to_string())
            .with_detail("duration", duration.value())
        })
    }
}

/// Gives one identifier newtype a WIT record counterpart in both directions.
///
/// The identifiers are separate records rather than one `id` record so the
/// generated bindings keep them apart: a guest cannot hand a `track-id` to a
/// function that wants a `clip-id`, on either side of the boundary.
macro_rules! id_conversions {
    ($($model:ty => $wit:ty),* $(,)?) => {
        $(
            impl From<$model> for $wit {
                fn from(id: $model) -> Self {
                    Self { value: id.to_string() }
                }
            }

            impl TryFrom<&$wit> for $model {
                type Error = SubError;

                /// # Errors
                ///
                /// Returns `model.invalid_id` if the text is not a UUID.
                fn try_from(id: &$wit) -> SubResult<Self> {
                    Self::parse(&id.value)
                }
            }

            impl TryFrom<$wit> for $model {
                type Error = SubError;

                /// # Errors
                ///
                /// Returns `model.invalid_id` if the text is not a UUID.
                fn try_from(id: $wit) -> SubResult<Self> {
                    Self::parse(&id.value)
                }
            }
        )*
    };
}

id_conversions! {
    ProjectId => wit_types::ProjectId,
    SequenceId => wit_types::SequenceId,
    TrackId => wit_types::TrackId,
    ClipId => wit_types::ClipId,
    MarkerId => wit_types::MarkerId,
    MediaId => wit_types::MediaId,
}

impl From<TrackKind> for wit_api::TrackKind {
    fn from(kind: TrackKind) -> Self {
        match kind {
            TrackKind::Video => Self::Video,
            TrackKind::Audio => Self::Audio,
        }
    }
}

impl From<wit_api::TrackKind> for TrackKind {
    fn from(kind: wit_api::TrackKind) -> Self {
        match kind {
            wit_api::TrackKind::Video => Self::Video,
            wit_api::TrackKind::Audio => Self::Audio,
        }
    }
}

impl From<Resolution> for wit_api::Resolution {
    fn from(resolution: Resolution) -> Self {
        Self {
            width: resolution.width(),
            height: resolution.height(),
        }
    }
}

impl TryFrom<wit_api::Resolution> for Resolution {
    type Error = SubError;

    /// # Errors
    ///
    /// Returns `model.invalid_settings` if either dimension is zero.
    fn try_from(resolution: wit_api::Resolution) -> SubResult<Self> {
        Self::new(resolution.width, resolution.height)
    }
}

/// Builds the `project-metadata` record a plugin reads through `project-info`.
///
/// `revision` is the engine revision the project was read at, so a plugin that
/// reads, decides and then writes can tell whether the project moved underneath
/// it.
#[must_use]
pub fn project_metadata(project: &Project, revision: u64) -> wit_api::ProjectMetadata {
    wit_api::ProjectMetadata {
        id: project.id.into(),
        name: project.name.clone(),
        revision,
        sequences: project
            .sequences
            .iter()
            .map(|sequence| sequence.id.into())
            .collect(),
        media_count: u32::try_from(project.media.len()).unwrap_or(u32::MAX),
    }
}

/// Builds the `sequence-metadata` record a plugin reads through `sequences`.
///
/// The duration is the end of the longest track, computed at the sequence's own
/// timebase so it is exact.
#[must_use]
pub fn sequence_metadata(sequence: &Sequence) -> wit_api::SequenceMetadata {
    let rate = sequence.settings.frame_rate;
    let duration = sequence
        .tracks
        .iter()
        .map(|track| track.duration(rate))
        .max()
        .unwrap_or_else(|| RationalTime::zero(rate));
    wit_api::SequenceMetadata {
        id: sequence.id.into(),
        name: sequence.name.clone(),
        frame_rate: rate.into(),
        resolution: sequence.settings.resolution.into(),
        sample_rate: sequence.settings.sample_rate,
        duration: duration.into(),
        track_count: u32::try_from(sequence.tracks.len()).unwrap_or(u32::MAX),
    }
}

/// Builds the `track-metadata` record a plugin reads through `tracks`.
///
/// `rate` is the sequence timebase the track is placed at; a track has no rate
/// of its own.
#[must_use]
pub fn track_metadata(track: &Track, rate: Rational) -> wit_api::TrackMetadata {
    wit_api::TrackMetadata {
        id: track.id.into(),
        name: track.name.clone(),
        kind: track.kind.into(),
        muted: track.muted,
        locked: track.locked,
        clip_count: u32::try_from(track.clips().count()).unwrap_or(u32::MAX),
        duration: track.duration(rate).into(),
    }
}

/// Builds the `clip-metadata` records a plugin reads through `clips`.
///
/// `rate` is the sequence timebase; both spans are exact at it. Gaps and
/// transitions are skipped, so the returned order is the clip order on the
/// track, not the item order.
#[must_use]
pub fn track_clip_metadata(track: &Track, rate: Rational) -> Vec<wit_api::ClipMetadata> {
    track
        .clip_placements(rate)
        .map(|(clip, timeline_range)| wit_api::ClipMetadata {
            id: clip.id.into(),
            name: clip.name.clone(),
            media: clip.media.into(),
            source_range: clip.source_range.into(),
            timeline_range: timeline_range.into(),
        })
        .collect()
}

/// Builds the `marker-metadata` record a plugin reads through `markers`.
#[must_use]
pub fn marker_metadata(marker: &Marker) -> wit_api::MarkerMetadata {
    wit_api::MarkerMetadata {
        id: marker.id.into(),
        name: marker.name.clone(),
        note: marker.note.clone(),
        marked_range: marker.marked_range.into(),
    }
}

#[cfg(test)]
mod tests {
    use sub_core::ErrorCode;
    use sub_model::{Clip, ColorTags, MediaPath, SequenceSettings};

    use super::*;

    fn fps() -> Rational {
        Rational::FPS_23_976
    }

    #[test]
    fn rational_time_round_trips_at_a_non_integral_rate() {
        let time = RationalTime::from_frames(1001, fps());
        let crossed = wit_types::RationalTime::from(time);
        assert_eq!(crossed.value, 1001);
        assert_eq!(crossed.rate.numerator, 24_000);
        assert_eq!(crossed.rate.denominator, 1_001);
        assert_eq!(RationalTime::try_from(crossed).unwrap(), time);
    }

    #[test]
    fn a_zero_rate_is_rejected_rather_than_trusted() {
        for rate in [
            wit_types::Rational {
                numerator: 0,
                denominator: 1,
            },
            wit_types::Rational {
                numerator: 24,
                denominator: 0,
            },
        ] {
            let err = Rational::try_from(rate).unwrap_err();
            assert_eq!(err.code, codes::INVALID_RATIONAL);
            let err =
                RationalTime::try_from(wit_types::RationalTime { value: 1, rate }).unwrap_err();
            assert_eq!(err.code, codes::INVALID_RATIONAL);
        }
    }

    #[test]
    fn time_range_round_trips_and_rejects_a_negative_duration() {
        let range = TimeRange::new(
            RationalTime::from_frames(10, fps()),
            RationalTime::from_frames(5, fps()),
        )
        .unwrap();
        let crossed = wit_types::TimeRange::from(range);
        assert_eq!(TimeRange::try_from(crossed).unwrap(), range);

        let negative = wit_types::TimeRange {
            start: RationalTime::from_frames(10, fps()).into(),
            duration: RationalTime::from_frames(-1, fps()).into(),
        };
        assert_eq!(
            TimeRange::try_from(negative).unwrap_err().code,
            codes::INVALID_TIME_RANGE
        );
    }

    #[test]
    fn errors_cross_out_and_back_without_losing_details() {
        let original = SubError::new(ErrorCode::from_static("plugin.command_rejected"), "no")
            .with_detail("method", "sequence.add_marker")
            .with_detail("count", 3);
        let crossed = wit_types::Error::from(&original);
        assert_eq!(crossed.code, "plugin.command_rejected");
        assert_eq!(crossed.details.len(), 2);

        let lifted = SubError::from(crossed);
        assert_eq!(lifted.code, original.code);
        assert_eq!(lifted.message, original.message);
        assert_eq!(lifted.details, original.details);
    }

    #[test]
    fn an_unparsable_plugin_error_code_is_kept_as_a_detail() {
        let lifted = SubError::from(wit_types::Error {
            code: "NOT A CODE".to_owned(),
            message: "boom".to_owned(),
            details: vec![wit_types::Detail {
                key: "raw".to_owned(),
                value: "not json".to_owned(),
            }],
        });
        assert_eq!(lifted.code, codes::INVALID_ERROR_CODE);
        assert_eq!(lifted.details["plugin_code"], "NOT A CODE");
        assert_eq!(lifted.details["raw"], "not json");
    }

    #[test]
    fn identifiers_round_trip_and_reject_rubbish() {
        let id = ClipId::new();
        let crossed = wit_types::ClipId::from(id);
        assert_eq!(ClipId::try_from(&crossed).unwrap(), id);
        assert_eq!(crossed.value, id.to_string());

        let err = TrackId::try_from(wit_types::TrackId {
            value: "not-a-uuid".to_owned(),
        })
        .unwrap_err();
        assert_eq!(err.code.as_str(), "model.invalid_id");
    }

    fn fixture() -> Project {
        let mut project = Project::new("Fixture");
        let settings =
            SequenceSettings::new(Resolution::HD_1080, fps(), 48_000, ColorTags::default())
                .unwrap();
        let mut sequence = Sequence::new("Seq 1", settings);
        let mut track = Track::new("V1", TrackKind::Video);
        let media = sub_model::MediaItem::new(MediaPath::new("media/clip.mov").unwrap());
        let range = TimeRange::new(
            RationalTime::zero(fps()),
            RationalTime::from_frames(24, fps()),
        )
        .unwrap();
        track
            .items
            .push(sub_model::TrackItem::Clip(Clip::new("A", media.id, range)));
        project.media.push(media);
        sequence.tracks.push(track);
        project.sequences.push(sequence);
        project
    }

    #[test]
    fn metadata_records_describe_the_project_exactly() {
        let project = fixture();
        let metadata = project_metadata(&project, 7);
        assert_eq!(metadata.id.value, project.id.to_string());
        assert_eq!(metadata.name, "Fixture");
        assert_eq!(metadata.revision, 7);
        assert_eq!(metadata.media_count, 1);
        assert_eq!(metadata.sequences.len(), 1);

        let sequence = &project.sequences[0];
        let sequence_metadata = sequence_metadata(sequence);
        assert_eq!(sequence_metadata.frame_rate.numerator, 24_000);
        assert_eq!(sequence_metadata.frame_rate.denominator, 1_001);
        assert_eq!(sequence_metadata.resolution.width, 1920);
        assert_eq!(sequence_metadata.sample_rate, 48_000);
        assert_eq!(sequence_metadata.track_count, 1);
        assert_eq!(
            RationalTime::try_from(sequence_metadata.duration).unwrap(),
            RationalTime::from_frames(24, fps())
        );

        let track = track_metadata(&sequence.tracks[0], fps());
        assert_eq!(track.kind, wit_api::TrackKind::Video);
        assert_eq!(TrackKind::from(track.kind), TrackKind::Video);
        assert_eq!(track.clip_count, 1);
        assert!(!track.muted);
        assert!(!track.locked);
        assert_eq!(
            RationalTime::try_from(track.duration).unwrap(),
            RationalTime::from_frames(24, fps())
        );
    }

    #[test]
    fn a_resolution_crosses_both_ways() {
        let crossed = wit_api::Resolution::from(Resolution::UHD_2160);
        assert_eq!((crossed.width, crossed.height), (3840, 2160));
        assert_eq!(Resolution::try_from(crossed).unwrap(), Resolution::UHD_2160);
        assert!(
            Resolution::try_from(wit_api::Resolution {
                width: 0,
                height: 1
            })
            .is_err()
        );
    }
}
