//! Typed, stable identifiers for every entity in the project model.
//!
//! Every ID is a distinct newtype around a `UUIDv7` so a `ClipId` can never be
//! passed where a `TrackId` is expected, and so an ID minted today keeps its
//! meaning across save, load, undo and a plugin or MCP round-trip
//! (docs/PLAN.md §5.6). Version 7 is chosen over version 4 because its leading
//! 48 bits are a Unix millisecond timestamp: IDs sort by creation time, which
//! keeps `BTreeMap` iteration and golden-file diffs stable and readable.
//!
//! The wire form is always the canonical lowercase hyphenated string, in every
//! serde format, human-readable or not.
//!
//! ```
//! use sub_model::{ClipId, TrackId};
//!
//! let clip = ClipId::new();
//! let text = clip.to_string();
//! assert_eq!(ClipId::parse(&text).unwrap(), clip);
//! // Distinct types: this would not compile.
//! // let track: TrackId = clip;
//! assert!(TrackId::parse("not-a-uuid").is_err());
//! ```

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sub_core::{SubError, SubResult};
use uuid::Uuid;

use crate::codes;

/// Defines one identifier newtype: a `UUIDv7` wrapper with string serde.
macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $entity:literal) => {
        $(#[$meta])*
        ///
        /// A `UUIDv7` newtype. Ordering follows creation time; the serde form is
        /// the canonical lowercase hyphenated string.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        impl $name {
            /// The name of the entity this identifies, used in error details.
            pub const ENTITY: &'static str = $entity;

            /// Mints a fresh identifier from the current time.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wraps an existing UUID, for tests, migrations and importers that
            /// carry identity in from elsewhere.
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The underlying UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            /// Parses the canonical string form.
            ///
            /// # Errors
            ///
            /// Returns a [`SubError`] with code `model.invalid_id` when `text`
            /// is not a UUID.
            pub fn parse(text: &str) -> SubResult<Self> {
                match Uuid::parse_str(text) {
                    Ok(uuid) => Ok(Self(uuid)),
                    Err(err) => Err(SubError::wrap(
                        codes::INVALID_ID,
                        format!("{} is not a valid {} id", text, $entity),
                        &err,
                    )
                    .with_detail("entity", $entity)
                    .with_detail("value", text)),
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut buf = Uuid::encode_buffer();
                f.write_str(self.0.hyphenated().encode_lower(&mut buf))
            }
        }

        impl FromStr for $name {
            type Err = SubError;

            fn from_str(text: &str) -> SubResult<Self> {
                Self::parse(text)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut buf = Uuid::encode_buffer();
                serializer.serialize_str(self.0.hyphenated().encode_lower(&mut buf))
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = Cow::<'de, str>::deserialize(deserializer)?;
                Self::parse(text.as_ref()).map_err(D::Error::custom)
            }
        }

        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                std::borrow::Cow::Borrowed(stringify!($name))
            }

            fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({
                    "type": "string",
                    "format": "uuid",
                    "description": concat!(
                        "The canonical lowercase hyphenated UUIDv7 of a ",
                        $entity,
                        "."
                    ),
                })
            }
        }
    };
}

define_id!(
    /// Identifies a [`Project`](crate::Project).
    ProjectId,
    "project"
);
define_id!(
    /// Identifies a [`Sequence`](crate::Sequence).
    SequenceId,
    "sequence"
);
define_id!(
    /// Identifies a [`Track`](crate::Track).
    TrackId,
    "track"
);
define_id!(
    /// Identifies a [`Clip`](crate::Clip).
    ClipId,
    "clip"
);
define_id!(
    /// Identifies a [`MediaItem`](crate::MediaItem).
    MediaId,
    "media"
);
define_id!(
    /// Identifies a [`Bin`](crate::Bin).
    BinId,
    "bin"
);
define_id!(
    /// Identifies a [`Marker`](crate::Marker).
    MarkerId,
    "marker"
);
define_id!(
    /// Identifies a [`ClipEffect`](crate::ClipEffect) in a clip's effect
    /// stack.
    EffectId,
    "effect"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_ids_are_unique_and_version_7() {
        let a = ClipId::new();
        let b = ClipId::new();
        assert_ne!(a, b);
        assert_eq!(a.as_uuid().get_version_num(), 7);
    }

    #[test]
    fn ids_sort_by_creation_time() {
        let mut ids: Vec<TrackId> = (0..8).map(|_| TrackId::new()).collect();
        let minted = ids.clone();
        ids.sort_unstable();
        assert_eq!(ids, minted, "UUIDv7 ids must already be in creation order");
    }

    #[test]
    fn display_and_parse_round_trip() {
        let id = MediaId::new();
        let text = id.to_string();
        assert_eq!(text.len(), 36);
        assert_eq!(text, text.to_lowercase());
        assert_eq!(MediaId::parse(&text).unwrap(), id);
        assert_eq!(text.parse::<MediaId>().unwrap(), id);
    }

    #[test]
    fn parse_rejects_malformed_text_with_a_stable_code() {
        let err = BinId::parse("not-a-uuid").unwrap_err();
        assert_eq!(err.code, codes::INVALID_ID);
        assert_eq!(
            err.details
                .get("entity")
                .and_then(serde_json::Value::as_str),
            Some("bin")
        );
        assert!(err.cause.is_some());
        assert!(ProjectId::parse("").is_err());
    }

    #[test]
    fn serde_uses_the_canonical_string_form() {
        let id =
            SequenceId::from_uuid(Uuid::parse_str("018f3e1a-6c2b-7f00-8000-0123456789ab").unwrap());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#""018f3e1a-6c2b-7f00-8000-0123456789ab""#);
        assert_eq!(serde_json::from_str::<SequenceId>(&json).unwrap(), id);
    }

    #[test]
    fn deserializing_rejects_a_malformed_string() {
        let err = serde_json::from_str::<MarkerId>(r#""nope""#).unwrap_err();
        assert!(err.to_string().contains("marker"), "{err}");
    }

    #[test]
    fn distinct_entities_report_their_own_name() {
        assert_eq!(ProjectId::ENTITY, "project");
        assert_eq!(ClipId::ENTITY, "clip");
        assert_eq!(MarkerId::ENTITY, "marker");
    }
}
