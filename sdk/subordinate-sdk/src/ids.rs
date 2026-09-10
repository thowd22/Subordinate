//! Working with the identifier records.
//!
//! WIT gives each identifier its own record so a [`TrackId`] cannot be passed
//! where a [`ClipId`] is expected — the property that makes the boundary worth
//! having. The cost is that building one reads `ClipId { value: text }` and
//! reading one reads `id.value`, at every call site.
//!
//! [`Id`] restores the ergonomics without giving up the distinction: it is
//! implemented for all six records, so `ClipId::parse(text)` and `id.as_str()`
//! work uniformly, while the six types stay unrelated to each other.
//!
//! ```
//! use subordinate_sdk::{ClipId, Id, TrackId};
//!
//! let clip = ClipId::parse("018f3c2e-0000-7000-8000-000000000001");
//! let track = TrackId::parse("018f3c2e-0000-7000-8000-000000000002");
//! assert_eq!(clip.as_str(), "018f3c2e-0000-7000-8000-000000000001");
//! // `clip` and `track` are different types: neither can stand in for the other.
//! assert_ne!(clip.as_str(), track.as_str());
//! ```
//!
//! The SDK does not validate the text. Identifiers are minted by the host, and
//! a plugin's job is to pass back what it was given; a plugin that invents one
//! learns it was wrong from the host's `edit.unknown_clip` (or sibling) error,
//! which is the same answer any other malformed identifier would get.

use crate::bindings::subordinate::plugin::types::{
    ClipId, MarkerId, MediaId, ProjectId, SequenceId, TrackId,
};

/// The operations every identifier record shares.
///
/// Implemented for [`ProjectId`], [`SequenceId`], [`TrackId`], [`ClipId`],
/// [`MarkerId`] and [`MediaId`]. It deliberately does not unify them: the trait
/// is a shorthand, not a conversion between identifier kinds.
pub trait Id: Sized {
    /// Wraps `text` — the canonical lowercase hyphenated `UUIDv7` form the host
    /// hands out — as this kind of identifier.
    fn parse(text: impl Into<String>) -> Self;

    /// The identifier's text, as the Command API and the project file spell it.
    fn as_str(&self) -> &str;

    /// The identifier's text, consuming the record.
    fn into_string(self) -> String;
}

/// Implements [`Id`] for one identifier record.
macro_rules! impl_id {
    ($($id:ty),* $(,)?) => {$(
        impl Id for $id {
            fn parse(text: impl Into<String>) -> Self {
                Self { value: text.into() }
            }

            fn as_str(&self) -> &str {
                &self.value
            }

            fn into_string(self) -> String {
                self.value
            }
        }
    )*};
}

impl_id!(ProjectId, SequenceId, TrackId, ClipId, MarkerId, MediaId);

#[cfg(test)]
mod tests {
    use super::{ClipId, Id, MediaId, ProjectId};

    #[test]
    fn an_identifier_round_trips_through_its_text() {
        let id = ClipId::parse("018f3c2e-0000-7000-8000-000000000001");
        assert_eq!(id.as_str(), "018f3c2e-0000-7000-8000-000000000001");
        assert_eq!(
            ClipId::parse(id.clone().into_string()).as_str(),
            id.as_str()
        );
    }

    #[test]
    fn every_identifier_kind_implements_the_trait() {
        assert_eq!(ProjectId::parse("p").as_str(), "p");
        assert_eq!(MediaId::parse(String::from("m")).as_str(), "m");
    }
}
