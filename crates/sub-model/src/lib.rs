//! Project data model: projects, sequences, tracks, clips, media items and bins.
//!
//! The schema mirrors OpenTimelineIO's Timeline/Stack/Track/Clip/Gap/
//! Transition/Marker shape so a future OTIO export is trivial. Files are
//! deterministic JSON carrying a `schema_version` plus a migration registry
//! so old projects always open. See docs/PLAN.md §5.1 and §5.6.
//!
//! Two rules hold throughout:
//!
//! - Every entity carries a typed [`ids`] newtype backed by a `UUIDv7`, stable
//!   across save, load, undo and any plugin or MCP round-trip.
//! - Every position and duration is a `RationalTime` from `sub-time`. There is
//!   no float in this crate.
//!
//! ```
//! use sub_model::{Clip, MediaItem, Project, Sequence, SequenceSettings, Track, TrackKind};
//! use sub_time::{Rational, RationalTime, TimeRange};
//!
//! let mut project = Project::new("Doc cut");
//! let media = MediaItem::new("interview.mp4");
//! let media_id = media.id;
//! project.media.push(media);
//!
//! let rate = Rational::FPS_24;
//! let source = TimeRange::new(RationalTime::zero(rate), RationalTime::new(48, rate)).unwrap();
//! let mut track = Track::new("V1", TrackKind::Video);
//! track.items.push(Clip::new("shot 1", media_id, source).into());
//!
//! let mut sequence = Sequence::new("Main", SequenceSettings::default());
//! sequence.tracks.push(track);
//! assert_eq!(sequence.tracks[0].duration(rate), RationalTime::new(48, rate));
//! project.sequences.push(sequence);
//! ```

pub mod ids;
pub mod marker;
pub mod media;
pub mod project;
pub mod sequence;
pub mod track;

pub use ids::{BinId, ClipId, MarkerId, MediaId, ProjectId, SequenceId, TrackId};
pub use marker::Marker;
pub use media::{Bin, MediaItem};
pub use project::Project;
pub use sequence::{
    ColorPrimaries, ColorSpace, ColorTags, Resolution, Sequence, SequenceSettings, TransferFunction,
};
pub use track::{Clip, Gap, Track, TrackItem, TrackKind, Transition};

/// The error codes this crate produces.
///
/// Codes are part of the public contract with agents and plugins: an existing
/// one is never renamed or given a new meaning (see `sub_core::error`).
pub mod codes {
    use sub_core::ErrorCode;

    /// A string is not a valid entity identifier.
    pub const INVALID_ID: ErrorCode = ErrorCode::from_static("model.invalid_id");
    /// Sequence settings are out of range (zero resolution or sample rate).
    pub const INVALID_SETTINGS: ErrorCode = ErrorCode::from_static("model.invalid_settings");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_live_in_the_model_domain() {
        for code in [codes::INVALID_ID, codes::INVALID_SETTINGS] {
            assert_eq!(code.domain(), "model");
            assert!(sub_core::ErrorCode::parse(code.as_str()).is_ok());
        }
    }
}
