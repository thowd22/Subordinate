//! Findings an analyzer plugin reported about one media item
//! (docs/PLAN.md §6.2).
//!
//! An analyzer never edits a project: it reads a media item and hands back
//! points, spans and scalars. Those land here, on the media item itself, so
//! they survive a save, travel with the project and can be reviewed before
//! anything moves on the timeline. Turning them into markers or cuts is a
//! separate undoable command.
//!
//! Everything is in **media time** — the timebase of the file — not the
//! timebase of any sequence a clip of it happens to sit on, so one analysis
//! stays correct for every clip cut from that media.
//!
//! ```
//! use sub_model::{Analysis, AnalysisRange, Marker};
//! use sub_time::{Rational, RationalTime, TimeRange};
//!
//! let rate = Rational::FPS_23_976;
//! let span = TimeRange::new(
//!     RationalTime::from_frames(24, rate),
//!     RationalTime::from_frames(12, rate),
//! )
//! .unwrap();
//!
//! let analysis = Analysis::new("silence")
//!     .with_markers(vec![Marker::new("silence", span)])
//!     .with_ranges(vec![AnalysisRange::new("silence", span)]);
//!
//! assert_eq!(analysis.analyzer, "silence");
//! assert_eq!(analysis.ranges_labelled("silence").count(), 1);
//! assert!(analysis.ranges_labelled("scene").next().is_none());
//! ```

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sub_time::TimeRange;

use crate::ids::MarkerId;
use crate::marker::Marker;

/// A labelled span of a media item: silence, a scene, a spoken sentence.
///
/// Ranges are kept apart from markers because they are the input to editing —
/// a cut-silence command consumes ranges — whereas markers exist for a person
/// to look at. A range carries a [`MarkerId`] so that turning it into a marker
/// produces the same identifier every time the command is replayed; the host
/// assigns it once, when the analyzer's findings are stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalysisRange {
    /// The identity a marker made from this range takes.
    pub id: MarkerId,
    /// What the span is, e.g. `silence` or `scene`. Analyzers keep a small,
    /// stable vocabulary: commands downstream match on it.
    pub label: String,
    /// The span, in media time.
    pub range: TimeRange,
}

impl AnalysisRange {
    /// A range labelled `label` covering `range`, with a fresh identifier.
    #[must_use]
    pub fn new(label: impl Into<String>, range: TimeRange) -> Self {
        Self {
            id: MarkerId::new(),
            label: label.into(),
            range,
        }
    }

    /// The marker this range becomes, named after its label.
    ///
    /// The identifier is the range's own, so the same range always yields the
    /// same marker: replaying the command that converts it is deterministic.
    #[must_use]
    pub fn to_marker(&self) -> Marker {
        Marker {
            id: self.id,
            name: self.label.clone(),
            marked_range: self.range,
            note: String::new(),
        }
    }
}

/// One analyzer run's findings about a media item.
///
/// `analyzer` is the identity of the analyzer that produced them, e.g.
/// `silence` or `scene-detect`. A media item holds at most one analysis per
/// analyzer: re-running one replaces what it found last time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Analysis {
    /// Which analyzer produced these findings.
    pub analyzer: String,
    /// Points and spans for a person to see, in media time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Spans for a command to act on, in media time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<AnalysisRange>,
    /// Scalar findings: loudness, a transcript, whatever the analyzer's own
    /// vocabulary holds. Ordered by key so the project file stays stable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

impl Analysis {
    /// An empty analysis attributed to `analyzer`.
    #[must_use]
    pub fn new(analyzer: impl Into<String>) -> Self {
        Self {
            analyzer: analyzer.into(),
            markers: Vec::new(),
            ranges: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }

    /// The same analysis carrying `markers`.
    #[must_use]
    pub fn with_markers(mut self, markers: Vec<Marker>) -> Self {
        self.markers = markers;
        self
    }

    /// The same analysis carrying `ranges`.
    #[must_use]
    pub fn with_ranges(mut self, ranges: Vec<AnalysisRange>) -> Self {
        self.ranges = ranges;
        self
    }

    /// The same analysis carrying `metadata`.
    #[must_use]
    pub fn with_metadata(mut self, metadata: BTreeMap<String, Value>) -> Self {
        self.metadata = metadata;
        self
    }

    /// True when the run found nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.markers.is_empty() && self.ranges.is_empty() && self.metadata.is_empty()
    }

    /// Every range with the label `label`, in the order the analyzer reported
    /// them.
    pub fn ranges_labelled<'a>(
        &'a self,
        label: &'a str,
    ) -> impl Iterator<Item = &'a AnalysisRange> + 'a {
        self.ranges.iter().filter(move |range| range.label == label)
    }

    /// Every finding as a marker: the markers as they are, then one marker per
    /// range named after its label.
    ///
    /// This is what `marker.from_analysis` copies onto a clip.
    #[must_use]
    pub fn to_markers(&self) -> Vec<Marker> {
        let mut markers = self.markers.clone();
        markers.extend(self.ranges.iter().map(AnalysisRange::to_marker));
        markers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_time::{Rational, RationalTime};

    fn span(start: i64, duration: i64) -> TimeRange {
        let rate = Rational::FPS_25;
        TimeRange::new(
            RationalTime::from_frames(start, rate),
            RationalTime::from_frames(duration, rate),
        )
        .unwrap()
    }

    #[test]
    fn a_range_always_becomes_the_same_marker() {
        let range = AnalysisRange::new("silence", span(10, 5));
        let first = range.to_marker();
        let second = range.to_marker();
        assert_eq!(first, second);
        assert_eq!(first.id, range.id);
        assert_eq!(first.name, "silence");
        assert_eq!(first.marked_range, range.range);
    }

    #[test]
    fn markers_come_before_ranges() {
        let analysis = Analysis::new("silence")
            .with_markers(vec![Marker::new("start", span(0, 0))])
            .with_ranges(vec![AnalysisRange::new("silence", span(10, 5))]);
        let markers = analysis.to_markers();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].name, "start");
        assert_eq!(markers[1].name, "silence");
        assert!(!analysis.is_empty());
    }

    #[test]
    fn an_analysis_with_nothing_in_it_serialises_to_its_name_alone() {
        let analysis = Analysis::new("loudness");
        assert!(analysis.is_empty());
        let text = serde_json::to_string(&analysis).unwrap();
        assert_eq!(text, r#"{"analyzer":"loudness"}"#);
        assert_eq!(
            serde_json::from_str::<Analysis>(&text).unwrap(),
            analysis,
            "the skipped fields default back"
        );
    }

    #[test]
    fn metadata_keeps_key_order() {
        let mut metadata = BTreeMap::new();
        metadata.insert("lufs".to_owned(), Value::from(-23));
        metadata.insert("channels".to_owned(), Value::from(2));
        let analysis = Analysis::new("loudness").with_metadata(metadata);
        let text = serde_json::to_string(&analysis).unwrap();
        assert_eq!(
            text,
            r#"{"analyzer":"loudness","metadata":{"channels":2,"lufs":-23}}"#
        );
    }
}
