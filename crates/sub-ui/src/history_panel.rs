//! The Edit menu and the history panel.
//!
//! The engine already keeps every edit as an undoable command
//! (docs/PLAN.md §5.1); what was missing was somewhere to see it. This module
//! is that view and nothing more: it reads a [`History`] into a
//! [`HistoryList`] of labels and a position, and it turns clicks into a
//! [`HistoryAction`] — a number of undo or redo steps — which the caller
//! performs through the Command API. No widget here touches a project.
//!
//! Jumping to a point in the panel is exactly that: walking the existing undo
//! and redo stacks until the position matches, so a jump is made of the same
//! steps a run of Ctrl+Z would take and stays undoable in the same way.
//!
//! ```
//! use sub_edit::History;
//! use sub_edit::commands::RenameSequence;
//! use sub_model::{Project, Sequence, SequenceSettings};
//! use sub_ui::history_panel::{HistoryAction, HistoryList};
//!
//! let mut project = Project::new("Doc cut");
//! let sequence = Sequence::new("Main", SequenceSettings::default());
//! let id = sequence.id;
//! project.sequences.push(sequence);
//!
//! let mut history = History::new();
//! for name in ["A", "B", "C"] {
//!     history
//!         .apply(&mut project, RenameSequence::new(id, name))
//!         .unwrap();
//! }
//!
//! let list = HistoryList::from_history(&history);
//! assert_eq!(list.position(), 3);
//! assert_eq!(list.labels().len(), 3);
//! assert!(list.undo_menu_label().starts_with("Undo "));
//!
//! // Jump back to just after the first command.
//! let action = HistoryAction::to_position(&list, 1).unwrap();
//! assert_eq!(action, HistoryAction::Undo { steps: 2 });
//! assert_eq!(action.perform(&mut history, &mut project).unwrap(), 2);
//! assert_eq!(project.sequences[0].name, "A");
//! ```

use eframe::egui::{self, Ui};
use sub_core::SubResult;
use sub_edit::{History, HistorySummary};
use sub_model::Project;

use crate::shortcuts::{Action, ShortcutMap};

/// The label the panel gives the state a project is in before its first
/// command.
pub const ORIGINAL_STATE_LABEL: &str = "Open state";

/// What an undone step further down the stack is called when the engine
/// reported only the depth of the stack and not every label.
pub const EARLIER_STEP_LABEL: &str = "Earlier edit";

/// What a redoable step further up the stack is called, for the same reason.
pub const LATER_STEP_LABEL: &str = "Later edit";

/// The history as the Edit menu and the history panel see it: every step's
/// label, oldest first, and how many of them are currently applied.
///
/// Entries before [`HistoryList::position`] are applied; entries from it
/// onwards have been undone and would be redone in list order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryList {
    labels: Vec<String>,
    position: usize,
}

impl HistoryList {
    /// The labels of a history's undo stack (oldest first) followed by its
    /// redo stack (in the order those steps would be redone).
    #[must_use]
    pub fn from_history(history: &History) -> Self {
        let mut labels: Vec<String> = history
            .undo_entries()
            .map(|entry| entry.label().to_owned())
            .collect();
        let position = labels.len();
        labels.extend(history.redo_entries().map(|entry| entry.label().to_owned()));
        Self { labels, position }
    }

    /// A list built straight from labels, for tests and for a caller that
    /// keeps its history on another thread.
    ///
    /// `position` is clamped to the number of labels, because a position past
    /// the end names a step that does not exist.
    #[must_use]
    pub fn new(labels: Vec<String>, position: usize) -> Self {
        let position = position.min(labels.len());
        Self { labels, position }
    }

    /// A list built from what the engine reports about its history.
    ///
    /// The engine's [`HistorySummary`] carries the two labels either side of
    /// the cursor and the depth of each stack, because that is all the Edit
    /// menu needs and all that can be read without copying the whole stack
    /// across the queue. The steps further away are therefore listed under a
    /// generic name: the menu never shows them, and the panel that does shows
    /// them as the anonymous steps they are.
    #[must_use]
    pub fn from_summary(summary: &HistorySummary) -> Self {
        let mut labels = Vec::with_capacity(summary.undo_len + summary.redo_len);
        for step in 0..summary.undo_len {
            let last = step + 1 == summary.undo_len;
            labels.push(match (last, summary.undo_label.as_deref()) {
                (true, Some(label)) => label.to_owned(),
                _ => EARLIER_STEP_LABEL.to_owned(),
            });
        }
        for step in 0..summary.redo_len {
            labels.push(match (step, summary.redo_label.as_deref()) {
                (0, Some(label)) => label.to_owned(),
                _ => LATER_STEP_LABEL.to_owned(),
            });
        }
        Self {
            labels,
            position: summary.undo_len,
        }
    }

    /// Every step, oldest first, undone ones included.
    #[must_use]
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// How many of the steps are applied: the number of undo steps available,
    /// and the position the panel marks as current.
    #[must_use]
    pub fn position(&self) -> usize {
        self.position
    }

    /// The number of steps in both stacks together.
    #[must_use]
    pub fn len(&self) -> usize {
        self.labels.len()
    }

    /// Whether the history holds no steps at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    /// Whether a step can be undone.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.position > 0
    }

    /// Whether a step can be redone.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.position < self.labels.len()
    }

    /// The name of the command an undo would reverse.
    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        self.position
            .checked_sub(1)
            .and_then(|index| self.labels.get(index))
            .map(String::as_str)
    }

    /// The name of the command a redo would replay.
    #[must_use]
    pub fn redo_label(&self) -> Option<&str> {
        self.labels.get(self.position).map(String::as_str)
    }

    /// What the Edit menu's undo entry reads: the command name when there is
    /// one, and the bare verb when there is nothing to undo.
    #[must_use]
    pub fn undo_menu_label(&self) -> String {
        menu_label("Undo", self.undo_label())
    }

    /// What the Edit menu's redo entry reads.
    #[must_use]
    pub fn redo_menu_label(&self) -> String {
        menu_label("Redo", self.redo_label())
    }

    /// Whether the step at `index` is currently applied.
    #[must_use]
    pub fn is_applied(&self, index: usize) -> bool {
        index < self.position
    }
}

/// "Undo Trim clip", or "Undo" when the stack is empty.
fn menu_label(verb: &str, command: Option<&str>) -> String {
    match command {
        Some(name) => format!("{verb} {name}"),
        None => verb.to_owned(),
    }
}

/// What the Edit menu or the history panel asks the Command API to do.
///
/// A jump is expressed as a run of steps rather than a target index so the
/// caller needs no history of its own: it undoes or redoes that many times,
/// through the same engine calls a keystroke would use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAction {
    /// Undo this many steps, most recent first.
    Undo {
        /// How many steps to reverse. Never zero.
        steps: usize,
    },
    /// Redo this many steps, oldest first.
    Redo {
        /// How many steps to replay. Never zero.
        steps: usize,
    },
}

impl HistoryAction {
    /// The action that moves `list` to `position`, or `None` when the history
    /// is already there or `position` is past the end of the list.
    #[must_use]
    pub fn to_position(list: &HistoryList, position: usize) -> Option<Self> {
        if position > list.len() {
            return None;
        }
        match position.cmp(&list.position()) {
            std::cmp::Ordering::Less => Some(Self::Undo {
                steps: list.position() - position,
            }),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(Self::Redo {
                steps: position - list.position(),
            }),
        }
    }

    /// How many steps the action moves.
    #[must_use]
    pub fn steps(self) -> usize {
        match self {
            Self::Undo { steps } | Self::Redo { steps } => steps,
        }
    }

    /// Performs the action against a history the caller owns, returning the
    /// number of steps that actually moved.
    ///
    /// A stack that runs out stops the walk rather than failing: the panel may
    /// have been drawn from a list that is one command out of date.
    ///
    /// # Errors
    ///
    /// Whatever [`History::undo`] or [`History::redo`] returns: `edit.group_open`
    /// while a command group is open, or `core.internal` when a command in the
    /// history cannot be replayed.
    pub fn perform(self, history: &mut History, project: &mut Project) -> SubResult<usize> {
        let mut moved = 0;
        for _ in 0..self.steps() {
            let stepped = match self {
                Self::Undo { .. } => history.undo(project)?,
                Self::Redo { .. } => history.redo(project)?,
            };
            if stepped.is_none() {
                break;
            }
            moved += 1;
        }
        Ok(moved)
    }
}

/// Draws the Edit menu, which carries Undo and Redo named after the command
/// they would reverse or replay.
///
/// Returns the single-step action the user chose, if any. Both entries are
/// disabled when their stack is empty, so a menu over an untouched project
/// offers nothing to click.
pub fn edit_menu_ui(
    ui: &mut Ui,
    list: &HistoryList,
    shortcuts: &ShortcutMap,
) -> Option<HistoryAction> {
    let mut action = None;
    ui.menu_button("Edit", |ui| {
        if menu_item(
            ui,
            &list.undo_menu_label(),
            list.can_undo(),
            shortcuts,
            Action::Undo,
        ) {
            action = Some(HistoryAction::Undo { steps: 1 });
            ui.close();
        }
        if menu_item(
            ui,
            &list.redo_menu_label(),
            list.can_redo(),
            shortcuts,
            Action::Redo,
        ) {
            action = Some(HistoryAction::Redo { steps: 1 });
            ui.close();
        }
    });
    action
}

/// One Edit menu entry: its label on the left, its chord on the right.
fn menu_item(
    ui: &mut Ui,
    label: &str,
    enabled: bool,
    shortcuts: &ShortcutMap,
    action: Action,
) -> bool {
    let button = egui::Button::new(label).shortcut_text(shortcuts.chord_label_for(action));
    ui.add_enabled(enabled, button).clicked()
}

/// The history panel: every step the history holds, with the current position
/// marked and any step clickable to jump there.
#[derive(Debug, Clone, Default)]
pub struct HistoryPanel {
    /// Whether the window is showing.
    pub open: bool,
    /// Where each row was drawn on the last frame, oldest first and starting
    /// with the open-state row. Kept for hit-testing and for tests that click
    /// a row by name.
    rows: Vec<egui::Rect>,
}

impl HistoryPanel {
    /// A closed panel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the panel if it is closed, and closes it if it is open.
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// The rect the row for `position` was last drawn at, where position 0 is
    /// the open state and position `n` is the point just after step `n - 1`.
    #[must_use]
    pub fn row_rect(&self, position: usize) -> Option<egui::Rect> {
        self.rows.get(position).copied()
    }

    /// Draws the panel as a window, if it is open.
    pub fn show(&mut self, ctx: &egui::Context, list: &HistoryList) -> Option<HistoryAction> {
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut action = None;
        egui::Window::new("History")
            .open(&mut open)
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                action = self.ui(ui, list);
            });
        self.open = open;
        action
    }

    /// Draws the panel's contents into an existing layout, so it can be
    /// exercised without a window.
    ///
    /// The list starts with the state the project was opened in, then one row
    /// per step. Clicking a row jumps to the point just after that step;
    /// clicking the first row undoes everything.
    pub fn ui(&mut self, ui: &mut Ui, list: &HistoryList) -> Option<HistoryAction> {
        let mut chosen = None;
        self.rows.clear();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let response = row_ui(ui, ORIGINAL_STATE_LABEL, list.position() == 0, true);
                self.rows.push(response.rect);
                if response.clicked() {
                    chosen = Some(0);
                }
                for (index, label) in list.labels().iter().enumerate() {
                    let position = index + 1;
                    let response = row_ui(
                        ui,
                        label,
                        list.position() == position,
                        list.is_applied(index),
                    );
                    self.rows.push(response.rect);
                    if response.clicked() {
                        chosen = Some(position);
                    }
                }
            });
        chosen.and_then(|position| HistoryAction::to_position(list, position))
    }
}

/// One row of the panel. An undone step is dimmed, the way the redo stack is
/// dimmed everywhere else.
fn row_ui(ui: &mut Ui, label: &str, current: bool, applied: bool) -> egui::Response {
    let text = if applied {
        egui::RichText::new(label)
    } else {
        egui::RichText::new(label).weak()
    };
    ui.selectable_label(current, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> HistoryList {
        HistoryList::new(
            vec![
                "Add clip".to_owned(),
                "Trim clip".to_owned(),
                "Move clip".to_owned(),
            ],
            2,
        )
    }

    #[test]
    fn the_menu_names_the_command_on_each_side_of_the_position() {
        let list = list();
        assert_eq!(list.undo_label(), Some("Trim clip"));
        assert_eq!(list.redo_label(), Some("Move clip"));
        assert_eq!(list.undo_menu_label(), "Undo Trim clip");
        assert_eq!(list.redo_menu_label(), "Redo Move clip");
        assert!(list.can_undo());
        assert!(list.can_redo());
        assert_eq!(list.len(), 3);
        assert!(!list.is_empty());
    }

    #[test]
    fn an_empty_history_leaves_the_menu_entries_bare() {
        let empty = HistoryList::default();
        assert!(empty.is_empty());
        assert!(!empty.can_undo());
        assert!(!empty.can_redo());
        assert_eq!(empty.undo_label(), None);
        assert_eq!(empty.redo_label(), None);
        assert_eq!(empty.undo_menu_label(), "Undo");
        assert_eq!(empty.redo_menu_label(), "Redo");
    }

    #[test]
    fn a_position_past_the_end_is_clamped() {
        let clamped = HistoryList::new(vec!["Add clip".to_owned()], 9);
        assert_eq!(clamped.position(), 1);
        assert!(!clamped.can_redo());
    }

    #[test]
    fn jumping_counts_the_steps_in_the_right_direction() {
        let list = list();
        assert_eq!(
            HistoryAction::to_position(&list, 0),
            Some(HistoryAction::Undo { steps: 2 })
        );
        assert_eq!(
            HistoryAction::to_position(&list, 3),
            Some(HistoryAction::Redo { steps: 1 })
        );
        assert_eq!(HistoryAction::to_position(&list, 2), None);
        assert_eq!(HistoryAction::to_position(&list, 4), None);
        assert_eq!(HistoryAction::Undo { steps: 2 }.steps(), 2);
        assert_eq!(HistoryAction::Redo { steps: 5 }.steps(), 5);
    }

    #[test]
    fn applied_steps_are_the_ones_before_the_position() {
        let list = list();
        assert!(list.is_applied(0));
        assert!(list.is_applied(1));
        assert!(!list.is_applied(2));
    }
}
