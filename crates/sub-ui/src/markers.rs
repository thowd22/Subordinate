//! Markers on the timeline: what the ruler shows, and what a gesture on one
//! asks the Command API to do.
//!
//! A marker is the simplest annotation an agent or an analyzer can leave on a
//! sequence, so the gestures over it are deliberately few: `M` drops one at
//! the playhead, a drag along the ruler moves it, a double-click renames it in
//! place and `Delete` removes it. Each of those is exactly one command in
//! [`sub_edit::commands::marker`], so the panel has no editing logic of its
//! own and every marker gesture is undoable — the same contract
//! [`TrackAction`](crate::track_header::TrackAction) keeps for the header
//! column.
//!
//! Time never becomes a float here. A drag is resolved to a pixel, the pixel
//! is turned into a [`RationalTime`] by [`TimelineView`](crate::timeline::TimelineView),
//! and a marker that covers a span keeps its exact duration when it moves: the
//! new range is `TimeRange::new(start, old.duration())`, never a rescaled
//! approximation of it.
//!
//! # Colour
//!
//! [`Marker`] carries no colour: decision-3 keeps the model's colour fields to
//! the colour-space tags on media and sequences, and adding a swatch to the
//! entity would be a project-file change rather than a UI one. So the ruler
//! colours a marker deterministically from its own identifier
//! ([`marker_color`]) out of [`MARKER_PALETTE`]. Two markers a user is likely
//! to compare rarely land on the same swatch, the colour survives save, load
//! and undo because the identifier does, and every machine painting the same
//! project paints the same ruler.

use eframe::egui::Color32;
use sub_edit::BoxedCommand;
use sub_edit::commands::{AddMarker, MarkerTarget, MoveMarker, RemoveMarker, RenameMarker};
use sub_model::{Marker, MarkerId, Sequence, SequenceId};
use sub_time::{RationalTime, TimeRange};

/// The swatches a marker on the ruler is drawn in.
///
/// Chosen to stay apart from the playhead's red and from the clip palette, so
/// a marker never reads as a clip or as the playhead, and to stay legible on
/// the dark ruler with black text over the flag.
pub const MARKER_PALETTE: [Color32; 6] = [
    Color32::from_rgb(240, 196, 64),  // amber
    Color32::from_rgb(96, 190, 232),  // sky
    Color32::from_rgb(126, 210, 128), // green
    Color32::from_rgb(198, 148, 232), // violet
    Color32::from_rgb(240, 152, 96),  // orange
    Color32::from_rgb(120, 208, 200), // teal
];

/// The name a marker dropped from the keyboard starts with.
///
/// A placeholder rather than a numbered one: the sequence may already hold a
/// `Marker 3`, and renaming is one double-click away.
pub const DEFAULT_MARKER_NAME: &str = "Marker";

/// The swatch `id` is painted in, the same on every machine and after every
/// reload.
///
/// The low byte of the identifier is used rather than the whole of it: a
/// `UUIDv7`'s leading bits are a millisecond timestamp, so markers dropped in
/// one session would otherwise all fall in the same swatch.
#[must_use]
pub fn marker_color(id: MarkerId) -> Color32 {
    let byte = id.as_uuid().as_bytes()[15];
    MARKER_PALETTE[usize::from(byte) % MARKER_PALETTE.len()]
}

/// What a gesture on a marker asks the Command API to do.
///
/// Each variant maps onto exactly one command in
/// [`sub_edit::commands::marker`], against the sequence's own marker list.
/// Clip markers use the same commands with a different
/// [`MarkerTarget`](sub_edit::commands::MarkerTarget); the ruler only ever
/// edits the sequence's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerAction {
    /// Drop a new marker at an instant on the sequence.
    Add {
        /// The marker to add, minted by the caller so the command replays
        /// byte for byte.
        marker: Marker,
    },
    /// Move a marker to another span.
    Move {
        /// The marker to move.
        marker: MarkerId,
        /// The span it ends up covering, its duration preserved.
        marked_range: TimeRange,
    },
    /// Give a marker a new name.
    Rename {
        /// The marker to rename.
        marker: MarkerId,
        /// Its new name.
        name: String,
    },
    /// Remove a marker.
    Remove {
        /// The marker to remove.
        marker: MarkerId,
    },
}

impl MarkerAction {
    /// A point marker named `name` dropped at `at`.
    #[must_use]
    pub fn add_at(at: RationalTime, name: impl Into<String>) -> Self {
        Self::Add {
            marker: Marker::new(name, TimeRange::empty_at(at)),
        }
    }

    /// The marker this action touches, when it touches an existing one.
    #[must_use]
    pub const fn marker(&self) -> MarkerId {
        match self {
            Self::Add { marker } => marker.id,
            Self::Move { marker, .. } | Self::Rename { marker, .. } | Self::Remove { marker } => {
                *marker
            }
        }
    }

    /// The command that performs this action on `sequence`.
    ///
    /// ```
    /// use sub_edit::History;
    /// use sub_model::{Project, Sequence, SequenceSettings};
    /// use sub_time::{Rational, RationalTime};
    /// use sub_ui::markers::MarkerAction;
    ///
    /// let mut project = Project::new("Doc cut");
    /// let sequence = Sequence::new("Main", SequenceSettings::default());
    /// let sequence_id = sequence.id;
    /// project.sequences.push(sequence);
    ///
    /// let at = RationalTime::new(48, Rational::FPS_24);
    /// let action = MarkerAction::add_at(at, "cut here");
    /// let mut history = History::new();
    /// history
    ///     .apply_boxed(&mut project, action.into_command(sequence_id))
    ///     .unwrap();
    /// assert_eq!(project.sequences[0].markers.len(), 1);
    ///
    /// history.undo(&mut project).unwrap();
    /// assert!(project.sequences[0].markers.is_empty());
    /// ```
    #[must_use]
    pub fn into_command(self, sequence: SequenceId) -> BoxedCommand {
        let target = MarkerTarget::sequence(sequence);
        match self {
            Self::Add { marker } => Box::new(AddMarker::new(target, marker)),
            Self::Move {
                marker,
                marked_range,
            } => Box::new(MoveMarker::new(target, marker, marked_range)),
            Self::Rename { marker, name } => Box::new(RenameMarker::new(target, marker, name)),
            Self::Remove { marker } => Box::new(RemoveMarker::new(target, marker)),
        }
    }
}

/// The span `marker` takes on when its head is dragged to `start`.
///
/// A point marker stays a point; a span marker keeps its exact duration, so
/// dragging the head of a five-second note along the ruler never stretches or
/// shortens it.
#[must_use]
pub fn moved_range(marker: &Marker, start: RationalTime) -> TimeRange {
    TimeRange::new(start, marker.marked_range.duration())
        .unwrap_or_else(|| TimeRange::empty_at(start))
}

/// The rename in progress on the ruler, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rename {
    /// The marker being renamed.
    marker: MarkerId,
    /// What has been typed so far.
    name: String,
    /// Whether the editor has yet to be handed the keyboard, in which case
    /// the name it was seeded with is still selected whole.
    fresh: bool,
}

/// The drag in progress on the ruler, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Drag {
    /// The marker under the pointer when the button went down.
    marker: MarkerId,
    /// Where its head sat then, so a drag that ends where it began raises no
    /// command and therefore no history entry.
    from: RationalTime,
    /// Where its head sits now.
    to: RationalTime,
}

/// The ruler's own marker state: what is selected, what is being dragged and
/// what is being renamed.
///
/// Everything else a marker shows is read from the [`Sequence`] each frame, so
/// the ruler cannot drift out of step with the project: a marker removed by an
/// undo simply stops being painted, and the selection that named it is dropped
/// by [`MarkerState::retain`].
#[derive(Debug, Default, Clone)]
pub struct MarkerState {
    /// The marker the keyboard acts on, when one is selected.
    selected: Option<MarkerId>,
    /// The drag in progress.
    drag: Option<Drag>,
    /// The rename in progress.
    rename: Option<Rename>,
}

impl MarkerState {
    /// A ruler with nothing selected, dragged or being renamed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The selected marker, if one is.
    #[must_use]
    pub const fn selected(&self) -> Option<MarkerId> {
        self.selected
    }

    /// Selects `marker`, or clears the selection with `None`.
    pub const fn select(&mut self, marker: Option<MarkerId>) {
        self.selected = marker;
    }

    /// The marker being dragged, if one is.
    #[must_use]
    pub const fn dragging(&self) -> Option<MarkerId> {
        match self.drag {
            Some(drag) => Some(drag.marker),
            None => None,
        }
    }

    /// Where the marker being dragged is being shown, which is not yet where
    /// the project has it.
    #[must_use]
    pub const fn drag_time(&self) -> Option<RationalTime> {
        match self.drag {
            Some(drag) => Some(drag.to),
            None => None,
        }
    }

    /// The marker whose name is being edited, if one is.
    #[must_use]
    pub const fn renaming(&self) -> Option<MarkerId> {
        match self.rename {
            Some(ref rename) => Some(rename.marker),
            None => None,
        }
    }

    /// What has been typed into the rename editor so far.
    #[must_use]
    pub fn rename_text(&self) -> Option<&str> {
        self.rename.as_ref().map(|rename| rename.name.as_str())
    }

    /// Starts dragging `marker`, whose head sits at `from`.
    pub const fn begin_drag(&mut self, marker: MarkerId, from: RationalTime) {
        self.selected = Some(marker);
        self.drag = Some(Drag {
            marker,
            from,
            to: from,
        });
    }

    /// Moves the drag in progress to `to`.
    pub const fn drag_to(&mut self, to: RationalTime) {
        if let Some(drag) = self.drag.as_mut() {
            drag.to = to;
        }
    }

    /// Ends the drag and returns the move it asks for.
    ///
    /// A drag that ends on the frame it started on is not an edit, so it
    /// raises no action and leaves the history alone.
    pub fn end_drag(&mut self, sequence: &Sequence) -> Option<MarkerAction> {
        let drag = self.drag.take()?;
        if drag.to == drag.from {
            return None;
        }
        let marker = sequence.marker(drag.marker)?;
        Some(MarkerAction::Move {
            marker: drag.marker,
            marked_range: moved_range(marker, drag.to),
        })
    }

    /// Abandons the drag in progress without moving anything.
    pub const fn cancel_drag(&mut self) {
        self.drag = None;
    }

    /// Opens the rename editor over `marker`, seeded with `name`.
    pub fn begin_rename(&mut self, marker: MarkerId, name: impl Into<String>) {
        self.selected = Some(marker);
        self.rename = Some(Rename {
            marker,
            name: name.into(),
            fresh: true,
        });
    }

    /// Closes the rename editor, discarding what was typed.
    pub fn cancel_rename(&mut self) {
        self.rename = None;
    }

    /// Closes the rename editor and returns the action that applies it.
    ///
    /// An empty or unchanged name is no edit at all, so it produces no action
    /// and therefore no history entry.
    pub fn commit_rename(&mut self, current: &str) -> Option<MarkerAction> {
        let rename = self.rename.take()?;
        let name = rename.name.trim();
        if name.is_empty() || name == current {
            return None;
        }
        Some(MarkerAction::Rename {
            marker: rename.marker,
            name: name.to_owned(),
        })
    }

    /// Removes the selected marker, if one is selected and nothing is being
    /// typed into.
    ///
    /// The selection is dropped straight away: the marker is about to stop
    /// existing, and a stale selection would let a second `Delete` ask to
    /// remove it again.
    pub fn remove_selected(&mut self) -> Option<MarkerAction> {
        if self.rename.is_some() {
            return None;
        }
        let marker = self.selected.take()?;
        self.drag = None;
        Some(MarkerAction::Remove { marker })
    }

    /// Drops any selection, drag or rename naming a marker `keep` rejects.
    ///
    /// Undoing the command that added a marker, or another agent removing it
    /// over the Command API, leaves the ruler holding an identifier the
    /// sequence no longer has; this is where that is noticed.
    pub fn retain(&mut self, keep: impl Fn(MarkerId) -> bool) {
        if self.selected.is_some_and(|marker| !keep(marker)) {
            self.selected = None;
        }
        if self.drag.is_some_and(|drag| !keep(drag.marker)) {
            self.drag = None;
        }
        if self
            .rename
            .as_ref()
            .is_some_and(|rename| !keep(rename.marker))
        {
            self.rename = None;
        }
    }

    /// The rename editor's text, mutably, for the widget to type into.
    pub(crate) fn rename_buffer(&mut self) -> Option<&mut String> {
        self.rename.as_mut().map(|rename| &mut rename.name)
    }

    /// Whether the rename editor has yet to take the keyboard.
    pub(crate) fn rename_is_fresh(&self) -> bool {
        self.rename.as_ref().is_some_and(|rename| rename.fresh)
    }

    /// Records that the rename editor now holds the keyboard.
    pub(crate) fn rename_focused(&mut self) {
        if let Some(rename) = self.rename.as_mut() {
            rename.fresh = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_model::SequenceSettings;
    use sub_time::Rational;

    const RATE: Rational = Rational::FPS_24;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, RATE)
    }

    fn sequence_with(markers: Vec<Marker>) -> Sequence {
        let mut sequence = Sequence::new("edit", SequenceSettings::default());
        sequence.markers = markers;
        sequence
    }

    #[test]
    fn a_markers_colour_is_stable_and_spread_over_the_palette() {
        let id = MarkerId::new();
        assert_eq!(marker_color(id), marker_color(id), "the same every frame");
        assert!(MARKER_PALETTE.contains(&marker_color(id)));

        // Markers minted in one session still spread over the whole palette:
        // the low byte selects the swatch, and a UUIDv7's low bytes are
        // random rather than the millisecond timestamp its leading bits are.
        let mut seen = Vec::new();
        for _ in 0..256 {
            let color = marker_color(MarkerId::new());
            if !seen.contains(&color) {
                seen.push(color);
            }
        }
        assert_eq!(
            seen.len(),
            MARKER_PALETTE.len(),
            "256 markers should reach every swatch"
        );
    }

    #[test]
    fn dragging_a_span_marker_keeps_its_duration() {
        let span = Marker::new(
            "review",
            TimeRange::new(frames(24), frames(48)).expect("a valid range"),
        );
        let moved = moved_range(&span, frames(96));
        assert_eq!(moved.start(), frames(96));
        assert_eq!(moved.duration(), frames(48), "the note is not stretched");

        let point = Marker::new("cue", TimeRange::empty_at(frames(24)));
        assert!(moved_range(&point, frames(96)).is_empty());
    }

    #[test]
    fn a_drag_that_ends_where_it_began_is_not_an_edit() {
        let marker = Marker::new("cue", TimeRange::empty_at(frames(24)));
        let id = marker.id;
        let sequence = sequence_with(vec![marker]);

        let mut state = MarkerState::new();
        state.begin_drag(id, frames(24));
        assert_eq!(state.dragging(), Some(id));
        assert_eq!(state.selected(), Some(id), "grabbing one selects it");
        state.drag_to(frames(24));
        assert_eq!(state.end_drag(&sequence), None);

        state.begin_drag(id, frames(24));
        state.drag_to(frames(72));
        assert_eq!(state.drag_time(), Some(frames(72)));
        assert_eq!(
            state.end_drag(&sequence),
            Some(MarkerAction::Move {
                marker: id,
                marked_range: TimeRange::empty_at(frames(72)),
            })
        );
        assert_eq!(state.dragging(), None, "the drag is over");
    }

    #[test]
    fn a_rename_commits_only_a_new_non_empty_name() {
        let marker = Marker::new("cue", TimeRange::empty_at(frames(0)));
        let id = marker.id;
        let mut state = MarkerState::new();

        state.begin_rename(id, "cue");
        assert_eq!(state.renaming(), Some(id));
        assert_eq!(state.rename_text(), Some("cue"));
        assert_eq!(state.commit_rename("cue"), None, "unchanged is no edit");

        state.begin_rename(id, "cue");
        *state.rename_buffer().expect("a rename is open") = "   ".to_owned();
        assert_eq!(state.commit_rename("cue"), None, "empty is no edit");

        state.begin_rename(id, "cue");
        *state.rename_buffer().expect("a rename is open") = "  reshoot ".to_owned();
        assert_eq!(
            state.commit_rename("cue"),
            Some(MarkerAction::Rename {
                marker: id,
                name: "reshoot".to_owned(),
            })
        );
        assert_eq!(state.renaming(), None);

        state.begin_rename(id, "cue");
        state.cancel_rename();
        assert_eq!(state.renaming(), None);
    }

    #[test]
    fn delete_removes_the_selection_but_not_while_a_name_is_being_typed() {
        let marker = Marker::new("cue", TimeRange::empty_at(frames(0)));
        let id = marker.id;
        let mut state = MarkerState::new();
        assert_eq!(state.remove_selected(), None, "nothing is selected");

        state.select(Some(id));
        state.begin_rename(id, "cue");
        assert_eq!(
            state.remove_selected(),
            None,
            "Delete belongs to the text editor while one is open"
        );

        state.cancel_rename();
        assert_eq!(
            state.remove_selected(),
            Some(MarkerAction::Remove { marker: id })
        );
        assert_eq!(state.selected(), None, "and the selection goes with it");
    }

    #[test]
    fn state_naming_a_marker_the_sequence_lost_is_dropped() {
        let marker = Marker::new("cue", TimeRange::empty_at(frames(0)));
        let id = marker.id;
        let mut state = MarkerState::new();
        state.begin_drag(id, frames(0));
        state.begin_rename(id, "cue");

        let empty = sequence_with(Vec::new());
        state.retain(|marker| empty.marker(marker).is_some());
        assert_eq!(state.selected(), None);
        assert_eq!(state.dragging(), None);
        assert_eq!(state.renaming(), None);
    }

    #[test]
    fn every_action_becomes_one_command() {
        let marker = Marker::new("cue", TimeRange::empty_at(frames(24)));
        let id = marker.id;
        let sequence = SequenceId::new();
        for (action, kind) in [
            (MarkerAction::Add { marker }, "marker.add"),
            (
                MarkerAction::Move {
                    marker: id,
                    marked_range: TimeRange::empty_at(frames(48)),
                },
                "marker.move",
            ),
            (
                MarkerAction::Rename {
                    marker: id,
                    name: "reshoot".to_owned(),
                },
                "marker.rename",
            ),
            (MarkerAction::Remove { marker: id }, "marker.remove"),
        ] {
            assert_eq!(action.marker(), id, "every action names the same marker");
            assert_eq!(action.into_command(sequence).kind(), kind);
        }
    }
}
