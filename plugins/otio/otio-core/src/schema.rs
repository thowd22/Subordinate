//! The OpenTimelineIO JSON schema, as far as interchange with the MVP model
//! needs it.
//!
//! OTIO's own reference implementation writes every object with an
//! `OTIO_SCHEMA` discriminator carrying a name and a version — `"Clip.1"`,
//! `"Clip.2"` — and readers are expected to accept older versions of a schema
//! they know. So these types:
//!
//! - **never** use `deny_unknown_fields`. A field this plugin does not model
//!   (`effects`, `available_image_bounds`, an application's own key) must ride
//!   through a read rather than fail it.
//! - default everything that OTIO treats as optional, so a hand-written or an
//!   older document that omits `enabled` or `markers` still reads.
//! - accept both `Clip.1`, which carries a single `media_reference`, and
//!   `Clip.2`, which carries a `media_references` map plus the key of the
//!   active one. Recent OTIO — and therefore Kdenlive, which links against
//!   the C++ library — writes `Clip.2`.
//!
//! Times are the one place floats appear; see [`crate::time`].

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Result, invalid_document};
use crate::time::{Rate, Span, Time};

/// The `OTIO_SCHEMA` value a timeline is written with.
pub const TIMELINE_SCHEMA: &str = "Timeline.1";
/// The `OTIO_SCHEMA` value a stack is written with.
pub const STACK_SCHEMA: &str = "Stack.1";
/// The `OTIO_SCHEMA` value a track is written with.
pub const TRACK_SCHEMA: &str = "Track.1";
/// The `OTIO_SCHEMA` value a clip is written with.
pub const CLIP_SCHEMA: &str = "Clip.1";
/// The `OTIO_SCHEMA` value a gap is written with.
pub const GAP_SCHEMA: &str = "Gap.1";
/// The `OTIO_SCHEMA` value a transition is written with.
pub const TRANSITION_SCHEMA: &str = "Transition.1";
/// The `OTIO_SCHEMA` value a marker is written with.
pub const MARKER_SCHEMA: &str = "Marker.2";
/// The `OTIO_SCHEMA` value an external media reference is written with.
pub const EXTERNAL_REFERENCE_SCHEMA: &str = "ExternalReference.1";
/// The `OTIO_SCHEMA` value a time is written with.
pub const RATIONAL_TIME_SCHEMA: &str = "RationalTime.1";
/// The `OTIO_SCHEMA` value a time range is written with.
pub const TIME_RANGE_SCHEMA: &str = "TimeRange.1";

/// The `Track.kind` string for picture.
pub const KIND_VIDEO: &str = "Video";
/// The `Track.kind` string for sound.
pub const KIND_AUDIO: &str = "Audio";

/// The `Transition.transition_type` a crossfade is written as.
pub const SMPTE_DISSOLVE: &str = "SMPTE_Dissolve";

/// The key `Clip.2` gives its single media reference.
pub const DEFAULT_MEDIA_KEY: &str = "DEFAULT_MEDIA";

/// The metadata namespace this plugin writes under, so a round trip through
/// Subordinate keeps what OTIO itself has no field for.
pub const METADATA_NAMESPACE: &str = "subordinate";

/// A serialised `RationalTime`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RationalTime {
    /// Always [`RATIONAL_TIME_SCHEMA`] on the way out.
    #[serde(
        rename = "OTIO_SCHEMA",
        default = "rational_time_schema",
        skip_deserializing
    )]
    pub schema: &'static str,
    /// Ticks per second, as a JSON number.
    pub rate: f64,
    /// The tick count, as a JSON number; always whole.
    pub value: f64,
}

fn rational_time_schema() -> &'static str {
    RATIONAL_TIME_SCHEMA
}

impl RationalTime {
    /// Writes an exact time in OTIO's encoding.
    #[must_use]
    pub fn from_time(time: Time) -> Self {
        Self {
            schema: RATIONAL_TIME_SCHEMA,
            rate: time.rate().as_f64(),
            value: time.as_f64(),
        }
    }

    /// Reads it back exactly.
    ///
    /// # Errors
    ///
    /// Returns `otio.invalid_time` when the rate is not an exact fraction or
    /// the value is not a whole number of ticks.
    pub fn to_time(self) -> Result<Time> {
        Time::from_f64(self.value, Rate::from_f64(self.rate)?)
    }
}

/// A serialised `TimeRange`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeRange {
    /// Always [`TIME_RANGE_SCHEMA`] on the way out.
    #[serde(
        rename = "OTIO_SCHEMA",
        default = "time_range_schema",
        skip_deserializing
    )]
    pub schema: &'static str,
    /// The first instant in the range.
    pub start_time: RationalTime,
    /// The length of the range.
    pub duration: RationalTime,
}

fn time_range_schema() -> &'static str {
    TIME_RANGE_SCHEMA
}

impl TimeRange {
    /// Writes an exact span in OTIO's encoding.
    #[must_use]
    pub fn from_span(span: Span) -> Self {
        Self {
            schema: TIME_RANGE_SCHEMA,
            start_time: RationalTime::from_time(span.start()),
            duration: RationalTime::from_time(span.duration()),
        }
    }

    /// Reads it back exactly.
    ///
    /// # Errors
    ///
    /// Returns `otio.invalid_time` when either end is not exact, or the
    /// duration is negative.
    pub fn to_span(self) -> Result<Span> {
        Span::new(self.start_time.to_time()?, self.duration.to_time()?)
    }
}

/// Free-form metadata: OTIO's `AnyDictionary`, which every object carries.
pub type Metadata = Map<String, Value>;

/// A media reference. Only `ExternalReference` names a file; a generator, a
/// missing reference and an absent one all describe a clip with no file
/// behind it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaReference {
    /// The reference's schema name and version, e.g. `ExternalReference.1`.
    #[serde(rename = "OTIO_SCHEMA", default)]
    pub schema: String,
    /// The file, when this is an external reference. OTIO writes a URL or a
    /// plain filesystem path here; Kdenlive writes the latter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_url: Option<String>,
    /// The portion of the file that exists, when the writer knew it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_range: Option<TimeRange>,
    /// The reference's own name; usually empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Anything the writer attached.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

impl MediaReference {
    /// An `ExternalReference` pointing at `target_url`.
    #[must_use]
    pub fn external(target_url: impl Into<String>) -> Self {
        Self {
            schema: EXTERNAL_REFERENCE_SCHEMA.to_owned(),
            target_url: Some(target_url.into()),
            available_range: None,
            name: String::new(),
            metadata: Metadata::new(),
        }
    }

    /// The file this reference names, if it names one.
    #[must_use]
    pub fn file(&self) -> Option<&str> {
        if self.schema.starts_with("ExternalReference") {
            self.target_url.as_deref()
        } else {
            None
        }
    }
}

/// A marker on a timeline, a track or a clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// Always [`MARKER_SCHEMA`] on the way out.
    #[serde(rename = "OTIO_SCHEMA", default = "marker_schema")]
    pub schema: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// The span covered, in the time of whatever holds the marker.
    pub marked_range: TimeRange,
    /// The marker's colour name, e.g. `RED`. Subordinate has no marker colour
    /// (decision-3 keeps colour tags out of the MVP), so it is written as
    /// OTIO's default and ignored on the way in.
    #[serde(default = "default_marker_color")]
    pub color: String,
    /// Free-form note. `Marker.1` had no such field, hence the default.
    #[serde(default)]
    pub comment: String,
    /// Anything the writer attached, such as Kdenlive's marker type.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

fn marker_schema() -> String {
    MARKER_SCHEMA.to_owned()
}

fn default_marker_color() -> String {
    "RED".to_owned()
}

/// One child of a track: OTIO's `Composable`.
///
/// The variant is chosen by the `OTIO_SCHEMA` name, so an unknown schema is a
/// document this plugin cannot read rather than a silently dropped item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "OTIO_SCHEMA")]
pub enum Item {
    /// A piece of media.
    ///
    /// Boxed: a clip is by far the largest item, and an unboxed one would make
    /// every gap in a long track as big as a clip.
    #[serde(rename = "Clip.1", alias = "Clip.2")]
    Clip(Box<Clip>),
    /// Empty time.
    #[serde(rename = "Gap.1")]
    Gap(Gap),
    /// A blend across the neighbouring cut.
    #[serde(rename = "Transition.1")]
    Transition(Transition),
}

/// A clip: a portion of one media reference, placed on a track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// The portion of the media used, in media time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_range: Option<TimeRange>,
    /// `Clip.1`'s single media reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_reference: Option<MediaReference>,
    /// `Clip.2`'s media reference map.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub media_references: Map<String, Value>,
    /// Which entry of `media_references` is the live one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_media_reference_key: Option<String>,
    /// Markers anchored to the clip, in source time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Whether the clip contributes to the composite.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Anything the writer attached.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

impl Clip {
    /// The media reference in force, whichever schema version wrote the clip.
    ///
    /// # Errors
    ///
    /// Returns `otio.invalid_document` when a `Clip.2` names an active key
    /// whose entry is not a media reference object.
    pub fn active_media_reference(&self) -> Result<Option<MediaReference>> {
        if let Some(reference) = &self.media_reference {
            return Ok(Some(reference.clone()));
        }
        if self.media_references.is_empty() {
            return Ok(None);
        }
        let key = self
            .active_media_reference_key
            .as_deref()
            .unwrap_or(DEFAULT_MEDIA_KEY);
        let Some(value) = self.media_references.get(key) else {
            return Ok(None);
        };
        if value.is_null() {
            return Ok(None);
        }
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| {
                invalid_document(
                    format!("clip media reference is not a media reference: {error}"),
                    "media_references",
                )
            })
    }
}

/// Empty time on a track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gap {
    /// Display name; usually empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The gap's length, carried as a range starting at zero.
    pub source_range: TimeRange,
    /// Anything the writer attached.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

/// A blend across the cut between the items either side of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// What kind of blend; only [`SMPTE_DISSOLVE`] is modelled.
    #[serde(default)]
    pub transition_type: String,
    /// How far the blend reaches back into the outgoing item.
    pub in_offset: RationalTime,
    /// How far it reaches forward into the incoming item.
    pub out_offset: RationalTime,
    /// Anything the writer attached.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

/// One lane of a timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Always [`TRACK_SCHEMA`] on the way out.
    #[serde(rename = "OTIO_SCHEMA", default = "track_schema")]
    pub schema: String,
    /// Display name, such as `V1`.
    #[serde(default)]
    pub name: String,
    /// [`KIND_VIDEO`], [`KIND_AUDIO`], or something this plugin skips.
    #[serde(default = "default_track_kind")]
    pub kind: String,
    /// The span the lane covers; Kdenlive writes it, plain OTIO often does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_range: Option<TimeRange>,
    /// The items, in playback order.
    #[serde(default)]
    pub children: Vec<Item>,
    /// Markers on the lane itself. Subordinate has no track markers, so these
    /// are not imported.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Whether the lane contributes; a muted Subordinate track is written as
    /// `false`.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Anything the writer attached.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

fn track_schema() -> String {
    TRACK_SCHEMA.to_owned()
}

fn default_track_kind() -> String {
    KIND_VIDEO.to_owned()
}

/// The stack of lanes a timeline holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    /// Always [`STACK_SCHEMA`] on the way out.
    #[serde(rename = "OTIO_SCHEMA", default = "stack_schema")]
    pub schema: String,
    /// Display name; OTIO writes `tracks`.
    #[serde(default = "default_stack_name")]
    pub name: String,
    /// The lanes, bottom-most first.
    #[serde(default)]
    pub children: Vec<Track>,
    /// Timeline-wide markers. Kdenlive puts its guides here, and so does this
    /// plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Whether the stack contributes.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Anything the writer attached.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

fn stack_schema() -> String {
    STACK_SCHEMA.to_owned()
}

fn default_stack_name() -> String {
    "tracks".to_owned()
}

/// A whole OTIO document: one timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// Always [`TIMELINE_SCHEMA`] on the way out.
    #[serde(rename = "OTIO_SCHEMA", default = "timeline_schema")]
    pub schema: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Where the timeline starts; zero for a Subordinate sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_start_time: Option<RationalTime>,
    /// The lanes.
    pub tracks: Stack,
    /// Anything the writer attached, such as Kdenlive's version.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

fn timeline_schema() -> String {
    TIMELINE_SCHEMA.to_owned()
}

const fn yes() -> bool {
    true
}

impl Timeline {
    /// Parses an OTIO JSON document.
    ///
    /// # Errors
    ///
    /// Returns `otio.invalid_document` when the text is not JSON, is not an
    /// object, or is not a `Timeline` this plugin can read.
    pub fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text)
            .map_err(|error| invalid_document(format!("not JSON: {error}"), "<document>"))?;
        let schema = value.get("OTIO_SCHEMA").and_then(Value::as_str);
        match schema {
            Some(name) if name.starts_with("Timeline") => {}
            Some(other) => {
                return Err(invalid_document(
                    format!("only a Timeline can be imported, not a {other}"),
                    "OTIO_SCHEMA",
                ));
            }
            None => {
                return Err(invalid_document(
                    "document has no OTIO_SCHEMA and so is not OpenTimelineIO",
                    "OTIO_SCHEMA",
                ));
            }
        }
        serde_json::from_value(value)
            .map_err(|error| invalid_document(error.to_string(), "<document>"))
    }

    /// Renders the document the way OTIO's own writer does: pretty-printed
    /// with a trailing newline, so a diff of two exports is readable.
    ///
    /// # Errors
    ///
    /// Returns `otio.invalid_document` if the document cannot be serialised,
    /// which only a non-finite number could cause.
    pub fn to_json(&self) -> Result<String> {
        let mut text = serde_json::to_string_pretty(self)
            .map_err(|error| invalid_document(error.to_string(), "<document>"))?;
        text.push('\n');
        Ok(text)
    }
}
