//! Just enough of the Subordinate project schema to export a sequence.
//!
//! The exporter reads the project through the Command API: `project.get`
//! returns the whole project in the shape the `.sub` file stores it. A plugin
//! is a sandboxed component and cannot depend on `sub-model`, so these types
//! mirror the fields the OTIO conversion needs and nothing else.
//!
//! Unknown fields are ignored on purpose: per-clip parameters, colour tags,
//! proxies, analyses and everything the schema grows later are none of this
//! plugin's business, and a new field in the project schema must not break an
//! export.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, codes};
use crate::time::{Rate, Span, Time};

/// The project as `project.get` returns it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// The project's identifier.
    #[serde(default)]
    pub id: String,
    /// The project's display name.
    #[serde(default)]
    pub name: String,
    /// Every source the project references.
    #[serde(default)]
    pub media: Vec<Media>,
    /// The sequences, in tab order.
    #[serde(default)]
    pub sequences: Vec<Sequence>,
}

impl Project {
    /// The sequence with this identifier or, failing that, this name.
    ///
    /// # Errors
    ///
    /// Returns `otio.unknown_sequence` when nothing matches.
    pub fn sequence(&self, wanted: &str) -> Result<&Sequence> {
        self.sequences
            .iter()
            .find(|sequence| sequence.id == wanted)
            .or_else(|| {
                self.sequences
                    .iter()
                    .find(|sequence| sequence.name == wanted)
            })
            .ok_or_else(|| {
                Error::new(
                    codes::UNKNOWN_SEQUENCE,
                    "the project holds no sequence with that identifier or name",
                )
                .with("sequence", wanted)
            })
    }

    /// The first sequence, for an export that named none.
    ///
    /// # Errors
    ///
    /// Returns `otio.unknown_sequence` when the project has no sequences.
    pub fn first_sequence(&self) -> Result<&Sequence> {
        self.sequences.first().ok_or_else(|| {
            Error::new(
                codes::UNKNOWN_SEQUENCE,
                "the project holds no sequences to export",
            )
        })
    }

    /// The media item with this identifier.
    #[must_use]
    pub fn media_item(&self, id: &str) -> Option<&Media> {
        self.media.iter().find(|item| item.id == id)
    }
}

/// One source file the project references.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Media {
    /// The media item's identifier.
    pub id: String,
    /// Display name, usually the file name.
    #[serde(default)]
    pub name: String,
    /// Where the file lives, relative to the project folder.
    #[serde(default)]
    pub path: String,
}

/// One editable timeline.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    /// The sequence's identifier.
    pub id: String,
    /// Display name, shown on the sequence tab.
    #[serde(default)]
    pub name: String,
    /// Canvas, timebase and audio rate.
    pub settings: Settings,
    /// Tracks, bottom-most first.
    #[serde(default)]
    pub tracks: Vec<Track>,
    /// Timeline-wide markers.
    #[serde(default)]
    pub markers: Vec<Marker>,
}

/// The canvas, timebase and audio rate of a sequence.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Render canvas size in pixels.
    pub resolution: Resolution,
    /// Editing timebase, exact.
    pub frame_rate: RateDoc,
    /// Audio sample rate in hertz.
    pub sample_rate: u32,
}

impl Default for Settings {
    /// 1920x1080 at 24 fps, 48 kHz audio: the model's own defaults.
    fn default() -> Self {
        Self {
            resolution: Resolution {
                width: 1920,
                height: 1080,
            },
            frame_rate: RateDoc {
                numerator: 24,
                denominator: 1,
            },
            sample_rate: 48_000,
        }
    }
}

/// A picture size in whole pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// A rate as the project file writes it: two integers, never a float.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateDoc {
    /// Units per `denominator` seconds.
    pub numerator: u32,
    /// Seconds per `numerator` units.
    pub denominator: u32,
}

impl RateDoc {
    /// The exact rate this pair stands for.
    ///
    /// # Errors
    ///
    /// Returns `otio.invalid_project` when either part is zero.
    pub fn to_rate(self) -> Result<Rate> {
        Rate::new(self.numerator, self.denominator).ok_or_else(|| {
            Error::new(codes::INVALID_PROJECT, "a rate may not have a zero part")
                .with("numerator", self.numerator)
                .with("denominator", self.denominator)
        })
    }
}

/// A time as the project file writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeDoc {
    /// The tick count.
    pub value: i64,
    /// The rate the ticks are counted at.
    pub rate: RateDoc,
}

impl TimeDoc {
    /// The exact time this pair stands for.
    ///
    /// # Errors
    ///
    /// Returns `otio.invalid_project` when the rate has a zero part.
    pub fn to_time(self) -> Result<Time> {
        Ok(Time::new(self.value, self.rate.to_rate()?))
    }
}

/// A span as the project file writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeDoc {
    /// The first instant in the span.
    pub start: TimeDoc,
    /// The length of the span.
    pub duration: TimeDoc,
}

impl RangeDoc {
    /// The exact span this pair stands for.
    ///
    /// # Errors
    ///
    /// Returns `otio.invalid_project` when a rate has a zero part, and
    /// `otio.invalid_time` when the duration is negative.
    pub fn to_span(self) -> Result<Span> {
        Span::new(self.start.to_time()?, self.duration.to_time()?)
    }
}

/// What a track carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Picture.
    Video,
    /// Sound.
    Audio,
}

/// One lane of a sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// The track's identifier.
    #[serde(default)]
    pub id: String,
    /// Display name, shown in the track header.
    #[serde(default)]
    pub name: String,
    /// Whether the lane carries picture or sound.
    pub kind: TrackKind,
    /// Whether the lane is silenced.
    #[serde(default)]
    pub muted: bool,
    /// Whether the lane is locked against edits.
    #[serde(default)]
    pub locked: bool,
    /// The items, in playback order.
    #[serde(default)]
    pub items: Vec<Item>,
}

/// One entry in a track's ordered child list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Item {
    /// A piece of media.
    Clip(Clip),
    /// Empty time.
    Gap(Gap),
    /// A blend across the neighbouring cut.
    Transition(Transition),
}

/// A slice of a media item placed on a track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// The clip's identifier.
    #[serde(default)]
    pub id: String,
    /// Display name, shown on the timeline rectangle.
    #[serde(default)]
    pub name: String,
    /// The media item this clip plays.
    pub media: String,
    /// The portion of the source used, in source time.
    pub source_range: RangeDoc,
    /// Markers anchored to this clip, in source time.
    #[serde(default)]
    pub markers: Vec<Marker>,
}

/// Empty time on a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    /// How long the gap lasts.
    pub duration: TimeDoc,
}

/// A blend across the cut between the items either side of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    /// A linear dissolve centred on the cut.
    Crossfade {
        /// How far the blend reaches back into the outgoing item.
        in_offset: TimeDoc,
        /// How far it reaches forward into the incoming item.
        out_offset: TimeDoc,
    },
}

/// A named annotation over a span of time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// The marker's identifier.
    #[serde(default)]
    pub id: String,
    /// Display name, shown on the ruler.
    #[serde(default)]
    pub name: String,
    /// The span covered.
    pub marked_range: RangeDoc,
    /// Free-form note.
    #[serde(default)]
    pub note: String,
}

/// Reads the `project` field of a `project.get` result.
///
/// The answer's `project` is the project *as the project file stores it*,
/// which is the file's whole document — `{"project": …, "schema_version": 1}`
/// — not the project object directly. Both shapes are accepted, because the
/// wrapper is the file format's and a plugin should not break if a future
/// method hands the project over unwrapped.
///
/// # Errors
///
/// Returns `otio.invalid_project` when the answer is not JSON, carries no
/// `project`, or carries one that is not the shape the project schema
/// promises.
pub fn parse_project_get(result: &str) -> Result<Project> {
    let answer: serde_json::Value = serde_json::from_str(result).map_err(|error| {
        Error::new(
            codes::INVALID_PROJECT,
            format!("project.get did not answer with JSON: {error}"),
        )
    })?;
    let mut project = answer.get("project").ok_or_else(|| {
        Error::new(
            codes::INVALID_PROJECT,
            "project.get answered without a project",
        )
    })?;
    if project.get("schema_version").is_some()
        && let Some(inner) = project.get("project")
    {
        project = inner;
    }
    serde_json::from_value(project.clone()).map_err(|error| {
        Error::new(
            codes::INVALID_PROJECT,
            format!("project.get answered with something that is not a project: {error}"),
        )
    })
}

/// Reads the exporter's arguments: the optional sequence and output path.
///
/// # Errors
///
/// Returns `otio.invalid_arguments` when the arguments are not a JSON object
/// of the documented shape.
pub fn parse_arguments(args: &str) -> Result<Arguments> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(Arguments::default());
    }
    serde_json::from_str(trimmed).map_err(|error| {
        Error::new(
            codes::INVALID_ARGUMENTS,
            format!("arguments must be a JSON object: {error}"),
        )
        .with(
            "hint",
            "{\"sequence\": \"<id or name>\", \"path\": \"/project/edit.otio\"}",
        )
    })
}

/// What the exporter was asked to do.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arguments {
    /// Which sequence to export, by identifier or by name. The first sequence
    /// when absent.
    #[serde(default)]
    pub sequence: Option<String>,
    /// Where to write the document, inside a folder the manifest granted write
    /// access to. The document is only returned, not written, when absent.
    #[serde(default)]
    pub path: Option<String>,
}
