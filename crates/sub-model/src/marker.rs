//! Markers: named points or ranges on a sequence or a clip.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_time::TimeRange;

use crate::ids::MarkerId;

/// A named annotation over a span of time.
///
/// OTIO counterpart: `Marker`, with `marked_range` and `name`. A point marker
/// is a marker whose range is empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    /// Stable identity, preserved across save, load and undo.
    pub id: MarkerId,
    /// Display name, shown on the ruler.
    pub name: String,
    /// The span the marker covers, in the time of whatever holds it: sequence
    /// time for a sequence marker, source time for a clip marker.
    pub marked_range: TimeRange,
    /// Free-form note, empty when the user has not written one.
    pub note: String,
}

impl Marker {
    /// Creates a marker with a fresh identifier and no note.
    #[must_use]
    pub fn new(name: impl Into<String>, marked_range: TimeRange) -> Self {
        Self {
            id: MarkerId::new(),
            name: name.into(),
            marked_range,
            note: String::new(),
        }
    }

    /// True when the marker is a point rather than a span.
    #[must_use]
    pub fn is_point(&self) -> bool {
        self.marked_range.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_time::{Rational, RationalTime};

    #[test]
    fn an_empty_range_is_a_point_marker() {
        let at = RationalTime::new(120, Rational::FPS_25);
        let point = Marker::new("cut here", TimeRange::empty_at(at));
        assert!(point.is_point());
        assert_eq!(point.marked_range.start(), at);
        assert!(point.note.is_empty());

        let span = Marker::new(
            "review",
            TimeRange::new(at, RationalTime::new(25, Rational::FPS_25)).unwrap(),
        );
        assert!(!span.is_point());
        assert_ne!(span.id, point.id);
    }
}
