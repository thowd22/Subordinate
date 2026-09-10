//! serde for the generated WIT value types.
//!
//! The Command API speaks JSON: `run-command` and `query` take a `params`
//! document and hand back a `result` document. The WIT boundary speaks records.
//! Without a bridge a plugin would hold a [`ClipId`] and still have to write
//! `format!(r#"{{"clip":"{}"}}"#, id.value)` by hand — which is exactly the
//! boilerplate this crate exists to remove.
//!
//! So [`serde::Serialize`] and [`serde::Deserialize`] are implemented here, by
//! hand, for the generated records. The generated types are local to this
//! crate, so the impls are allowed; they are written rather than derived
//! because `wit_bindgen::generate!` cannot be asked to derive them, and because
//! the JSON shape has to match the engine's own types exactly rather than
//! whatever a derive would pick:
//!
//! - every identifier is a plain JSON string, `"018f3c2e-…"`, the same form
//!   `sub_model`'s `UUIDv7` newtypes use, not `{ "value": "…" }`;
//! - a [`Rational`] is `{ "numerator": n, "denominator": d }` and is *not*
//!   reduced, because `30000/1001` and `60000/2002` are the same rate but not
//!   the same value;
//! - a [`RationalTime`] is `{ "value": v, "rate": { … } }`, two integers, so a
//!   time survives the round trip as the same instant at the same rate;
//! - a [`TimeRange`] is `{ "start": …, "duration": … }`.
//!
//! Nothing here rounds, and nothing here goes through `f64`.
//!
//! [`Error`] and [`Detail`] are serialised too, in the shape `SubError` uses on
//! the wire — `{ "code": …, "message": …, "details": { … } }` with the details
//! as a JSON object rather than a list of pairs — so a plugin can log a host
//! failure, or return one, without reshaping it.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::bindings::subordinate::plugin::command_api::TrackKind;
use crate::bindings::subordinate::plugin::types::{
    ClipId, Detail, Error, MarkerId, MediaId, ProjectId, Rational, RationalTime, SequenceId,
    TimeRange, TrackId,
};

/// Implements string serde for one identifier record.
macro_rules! id_serde {
    ($($id:ty),* $(,)?) => {$(
        impl Serialize for $id {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.value)
            }
        }

        impl<'de> Deserialize<'de> for $id {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                String::deserialize(deserializer).map(|value| Self { value })
            }
        }
    )*};
}

id_serde!(ProjectId, SequenceId, TrackId, ClipId, MarkerId, MediaId);

/// The JSON shape of a [`Rational`]: the two integers, unreduced.
#[derive(Serialize, Deserialize)]
struct RationalRepr {
    numerator: u32,
    denominator: u32,
}

impl Serialize for Rational {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RationalRepr {
            numerator: self.numerator,
            denominator: self.denominator,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Rational {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = RationalRepr::deserialize(deserializer)?;
        Ok(Self {
            numerator: repr.numerator,
            denominator: repr.denominator,
        })
    }
}

/// The JSON shape of a [`RationalTime`]: a tick count and its rate.
#[derive(Serialize, Deserialize)]
struct RationalTimeRepr {
    value: i64,
    rate: Rational,
}

impl Serialize for RationalTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RationalTimeRepr {
            value: self.value,
            rate: self.rate,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RationalTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = RationalTimeRepr::deserialize(deserializer)?;
        Ok(Self {
            value: repr.value,
            rate: repr.rate,
        })
    }
}

/// The JSON shape of a [`TimeRange`]: a start and a length.
#[derive(Serialize, Deserialize)]
struct TimeRangeRepr {
    start: RationalTime,
    duration: RationalTime,
}

impl Serialize for TimeRange {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        TimeRangeRepr {
            start: self.start,
            duration: self.duration,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TimeRange {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = TimeRangeRepr::deserialize(deserializer)?;
        Ok(Self {
            start: repr.start,
            duration: repr.duration,
        })
    }
}

impl Serialize for TrackKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::Video => "video",
            Self::Audio => "audio",
        })
    }
}

impl<'de> Deserialize<'de> for TrackKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "video" => Ok(Self::Video),
            "audio" => Ok(Self::Audio),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["video", "audio"],
            )),
        }
    }
}

/// A host failure serialises the way `SubError` does: the details are one JSON
/// object, not the list of pairs WIT has to carry them as.
impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("code", &self.code)?;
        map.serialize_entry("message", &self.message)?;
        map.serialize_entry("details", &DetailsMap(&self.details))?;
        map.end()
    }
}

/// The `details` object of a serialised [`Error`].
struct DetailsMap<'a>(&'a [Detail]);

impl Serialize for DetailsMap<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for detail in self.0 {
            map.serialize_entry(&detail.key, &detail.value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Error {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads `{ "code": …, "message": …, "details": { … } }`, tolerating a
        /// missing or null `details`: not every producer emits one.
        struct ErrorVisitor;

        impl<'de> Visitor<'de> for ErrorVisitor {
            type Value = Error;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a Subordinate error object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut code: Option<String> = None;
                let mut message: Option<String> = None;
                let mut details: Vec<Detail> = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "code" => code = Some(map.next_value()?),
                        "message" => message = Some(map.next_value()?),
                        "details" => {
                            details = map
                                .next_value::<Option<BTreeMap<String, String>>>()?
                                .unwrap_or_default()
                                .into_iter()
                                .map(|(key, value)| Detail { key, value })
                                .collect();
                        }
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(Error {
                    code: code.ok_or_else(|| serde::de::Error::missing_field("code"))?,
                    message: message.ok_or_else(|| serde::de::Error::missing_field("message"))?,
                    details,
                })
            }
        }

        deserializer.deserialize_map(ErrorVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::{ClipId, Error, Rational, RationalTime, TimeRange, TrackKind};
    use serde_json::{Value, json};

    #[test]
    fn an_identifier_is_a_plain_string() {
        let id = ClipId {
            value: "018f3c2e-0000-7000-8000-000000000001".to_owned(),
        };
        let json = serde_json::to_value(&id).unwrap();
        assert_eq!(json, json!("018f3c2e-0000-7000-8000-000000000001"));
        assert_eq!(
            serde_json::from_value::<ClipId>(json).unwrap().value,
            id.value
        );
    }

    #[test]
    fn a_time_keeps_its_unreduced_rate_through_a_round_trip() {
        let time = RationalTime {
            value: 48,
            rate: Rational {
                numerator: 60_000,
                denominator: 2_002,
            },
        };
        let json = serde_json::to_value(time).unwrap();
        assert_eq!(
            json,
            json!({ "value": 48, "rate": { "numerator": 60_000, "denominator": 2_002 } })
        );
        let back: RationalTime = serde_json::from_value(json).unwrap();
        assert_eq!(back.value, 48);
        assert_eq!(back.rate.numerator, 60_000);
        assert_eq!(back.rate.denominator, 2_002);
    }

    #[test]
    fn a_range_is_a_start_and_a_duration() {
        let rate = Rational {
            numerator: 24,
            denominator: 1,
        };
        let range = TimeRange {
            start: RationalTime { value: 10, rate },
            duration: RationalTime { value: 5, rate },
        };
        let json = serde_json::to_value(range).unwrap();
        assert_eq!(json["start"]["value"], json!(10));
        assert_eq!(json["duration"]["value"], json!(5));
        let back: TimeRange = serde_json::from_value(json).unwrap();
        assert_eq!(back.duration.value, 5);
    }

    #[test]
    fn a_track_kind_is_the_snake_case_word_the_engine_uses() {
        assert_eq!(
            serde_json::to_value(TrackKind::Video).unwrap(),
            json!("video")
        );
        assert!(matches!(
            serde_json::from_value::<TrackKind>(json!("audio")).unwrap(),
            TrackKind::Audio
        ));
        assert!(serde_json::from_value::<TrackKind>(json!("subtitle")).is_err());
    }

    #[test]
    fn an_error_carries_its_details_as_an_object() {
        let json = json!({
            "code": "edit.unknown_clip",
            "message": "no such clip",
            "details": { "clip": "\"018f\"" },
        });
        let error: Error = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(error.code, "edit.unknown_clip");
        assert_eq!(error.details.len(), 1);
        assert_eq!(error.details[0].key, "clip");
        assert_eq!(serde_json::to_value(&error).unwrap(), json);
    }

    #[test]
    fn an_error_without_details_still_reads() {
        let error: Error =
            serde_json::from_value(json!({ "code": "plugin.internal", "message": "boom" }))
                .unwrap();
        assert!(error.details.is_empty());
        assert_eq!(
            serde_json::to_value(&error).unwrap()["details"],
            Value::Object(serde_json::Map::new())
        );
    }
}
