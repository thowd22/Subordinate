//! The undo/redo stack.
//!
//! [`History`] owns nothing but commands: it applies a [`Command`], keeps the
//! [`Inverse`] the command returned, and moves entries between an undo and a
//! redo stack. The project itself is passed in on every call, because the
//! engine thread owns it (docs/PLAN.md §5.1).
//!
//! One entry is one step in the undo menu. An entry usually holds a single
//! command, but a [group](History::begin_group) collects several so that a
//! drag that moved twelve clips undoes in one keystroke.

use std::collections::VecDeque;

use sub_core::{SubError, SubResult, codes as core_codes};
use sub_model::Project;

use crate::codes;
use crate::command::{AnyCommand, BoxedCommand, Command, CommandEnvelope};

/// The number of undo steps a [`History::new`] keeps.
pub const DEFAULT_DEPTH: usize = 100;

/// One applied command and the command that undoes it.
#[derive(Debug)]
struct Step {
    forward: BoxedCommand,
    inverse: BoxedCommand,
}

/// One step in the undo menu: a label and the commands it applied, in order.
#[derive(Debug)]
pub struct HistoryEntry {
    label: String,
    steps: Vec<Step>,
}

impl HistoryEntry {
    /// The label shown in the undo menu and the history panel.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The number of commands the entry applied.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the entry applied no commands. Entries are never pushed empty,
    /// so this is always false for an entry the history holds.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The commands the entry applies when it is (re)done, in order.
    pub fn commands(&self) -> impl Iterator<Item = &dyn AnyCommand> + '_ {
        self.steps.iter().map(|step| step.forward.as_ref())
    }

    /// The entry as the envelopes the Command API sends and the log stores.
    ///
    /// # Errors
    ///
    /// Returns `edit.invalid_command` if one of the commands cannot be
    /// serialised.
    pub fn to_envelopes(&self) -> SubResult<Vec<CommandEnvelope>> {
        self.commands().map(AnyCommand::to_envelope).collect()
    }
}

/// A group being built by [`History::begin_group`].
#[derive(Debug)]
struct OpenGroup {
    label: String,
    steps: Vec<Step>,
}

/// The undo/redo stack for one project.
///
/// ```
/// # use serde::{Deserialize, Serialize};
/// # use sub_core::SubResult;
/// # use sub_edit::{Command, History, Inverse};
/// # use sub_model::Project;
/// # #[derive(Debug, Serialize, Deserialize)]
/// # struct RenameProject { name: String }
/// # impl Command for RenameProject {
/// #     const KIND: &'static str = "project.rename";
/// #     fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
/// #         let previous = std::mem::replace(&mut project.name, self.name.clone());
/// #         Ok(Inverse::new(RenameProject { name: previous }))
/// #     }
/// # }
/// let mut project = Project::new("Untitled");
/// let mut history = History::new();
///
/// history
///     .apply(&mut project, RenameProject { name: "Doc cut".to_owned() })
///     .unwrap();
/// assert_eq!(project.name, "Doc cut");
///
/// history.undo(&mut project).unwrap();
/// assert_eq!(project.name, "Untitled");
///
/// history.redo(&mut project).unwrap();
/// assert_eq!(project.name, "Doc cut");
/// ```
#[derive(Debug)]
pub struct History {
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    depth: usize,
    group: Option<OpenGroup>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            depth: DEFAULT_DEPTH,
            group: None,
        }
    }
}

impl History {
    /// An empty history keeping [`DEFAULT_DEPTH`] steps.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty history keeping `depth` steps.
    ///
    /// # Errors
    ///
    /// Returns `core.invalid_argument` when `depth` is zero: a history that
    /// remembers nothing would silently make edits unundoable.
    pub fn with_depth(depth: usize) -> SubResult<Self> {
        Ok(Self {
            depth: checked_depth(depth)?,
            ..Self::default()
        })
    }

    /// The number of undo steps kept.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Changes the depth, dropping the oldest steps if the new depth is
    /// smaller.
    ///
    /// # Errors
    ///
    /// Returns `core.invalid_argument` when `depth` is zero.
    pub fn set_depth(&mut self, depth: usize) -> SubResult<()> {
        self.depth = checked_depth(depth)?;
        self.trim();
        Ok(())
    }

    /// Applies a command and pushes it onto the undo stack, or onto the open
    /// group if there is one.
    ///
    /// Applying outside a group clears the redo stack, because the redone
    /// commands no longer describe this project.
    ///
    /// # Errors
    ///
    /// Whatever the command returns. Nothing is pushed in that case; inside a
    /// group, the commands already applied by that group are rolled back and
    /// the group is dropped (see [`History::begin_group`]).
    pub fn apply<C: Command>(&mut self, project: &mut Project, command: C) -> SubResult<()> {
        self.apply_boxed(project, Box::new(command))
    }

    /// Applies an already-boxed command, as decoded from a
    /// [`crate::CommandEnvelope`].
    ///
    /// # Errors
    ///
    /// The same as [`History::apply`].
    pub fn apply_boxed(&mut self, project: &mut Project, command: BoxedCommand) -> SubResult<()> {
        let label = command.label_erased();
        let step = match apply_step(project, command) {
            Ok(step) => step,
            Err(err) => {
                if self.group.is_some() {
                    self.abort_group(project)?;
                }
                return Err(err);
            }
        };

        if let Some(group) = &mut self.group {
            group.steps.push(step);
        } else {
            self.push_entry(HistoryEntry {
                label,
                steps: vec![step],
            });
        }
        Ok(())
    }

    /// Starts a group: every command applied until
    /// [`History::commit_group`] becomes one undo step labelled `label`.
    ///
    /// If a command inside the group fails, the group is rolled back
    /// automatically — the commands it already applied are undone in reverse
    /// order — so the project never keeps half a drag.
    ///
    /// # Errors
    ///
    /// Returns `edit.group_open` if a group is already open; groups do not
    /// nest.
    pub fn begin_group(&mut self, label: impl Into<String>) -> SubResult<()> {
        if self.group.is_some() {
            return Err(SubError::new(
                codes::GROUP_OPEN,
                "a command group is already open",
            ));
        }
        self.group = Some(OpenGroup {
            label: label.into(),
            steps: Vec::new(),
        });
        Ok(())
    }

    /// Closes the open group and pushes it as one undo step.
    ///
    /// Returns `false` when the group applied no commands: nothing is pushed
    /// and the undo stack is untouched.
    ///
    /// # Errors
    ///
    /// Returns `edit.no_group` when no group is open.
    pub fn commit_group(&mut self) -> SubResult<bool> {
        let group = self
            .group
            .take()
            .ok_or_else(|| SubError::new(codes::NO_GROUP, "no command group is open to commit"))?;
        if group.steps.is_empty() {
            return Ok(false);
        }
        self.push_entry(HistoryEntry {
            label: group.label,
            steps: group.steps,
        });
        Ok(true)
    }

    /// Closes the open group and undoes everything it applied, leaving the
    /// project as it was before [`History::begin_group`].
    ///
    /// # Errors
    ///
    /// - `edit.no_group` when no group is open.
    /// - `core.internal` when one of the inverses fails, which means a command
    ///   returned an inverse that does not apply and is a bug in that command.
    ///   The group is dropped either way.
    pub fn abort_group(&mut self, project: &mut Project) -> SubResult<()> {
        let group = self
            .group
            .take()
            .ok_or_else(|| SubError::new(codes::NO_GROUP, "no command group is open to abort"))?;
        rollback(project, group.steps)
    }

    /// Whether a group is open.
    #[must_use]
    pub fn in_group(&self) -> bool {
        self.group.is_some()
    }

    /// Undoes the most recent step, returning its label, or `None` when there
    /// is nothing to undo.
    ///
    /// # Errors
    ///
    /// - `edit.group_open` when a group is open: commit or abort it first.
    /// - `core.internal` when an inverse fails to apply. The entry is put back
    ///   on the undo stack, but the project may be partly undone, so the
    ///   engine should treat this as unrecoverable and reload the project.
    pub fn undo(&mut self, project: &mut Project) -> SubResult<Option<String>> {
        self.check_no_group("undo")?;
        let Some(mut entry) = self.undo.pop_back() else {
            return Ok(None);
        };
        match reverse(project, &mut entry, Direction::Undo) {
            Ok(()) => {
                let label = entry.label.clone();
                self.redo.push(entry);
                Ok(Some(label))
            }
            Err(err) => {
                self.undo.push_back(entry);
                Err(err)
            }
        }
    }

    /// Redoes the most recently undone step, returning its label, or `None`
    /// when there is nothing to redo.
    ///
    /// # Errors
    ///
    /// The same as [`History::undo`].
    pub fn redo(&mut self, project: &mut Project) -> SubResult<Option<String>> {
        self.check_no_group("redo")?;
        let Some(mut entry) = self.redo.pop() else {
            return Ok(None);
        };
        match reverse(project, &mut entry, Direction::Redo) {
            Ok(()) => {
                let label = entry.label.clone();
                self.undo.push_back(entry);
                Ok(Some(label))
            }
            Err(err) => {
                self.redo.push(entry);
                Err(err)
            }
        }
    }

    /// Forgets every step in both directions, leaving the project alone.
    ///
    /// An open group is dropped without being rolled back, so abort it first
    /// if the project still needs restoring.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.group = None;
    }

    /// Whether there is a step to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is a step to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The label of the step [`History::undo`] would undo.
    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.back().map(HistoryEntry::label)
    }

    /// The label of the step [`History::redo`] would redo.
    #[must_use]
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(HistoryEntry::label)
    }

    /// The undo steps, oldest first: what a history panel lists.
    pub fn undo_entries(&self) -> impl DoubleEndedIterator<Item = &HistoryEntry> + '_ {
        self.undo.iter()
    }

    /// The redo steps, most recently undone first.
    pub fn redo_entries(&self) -> impl DoubleEndedIterator<Item = &HistoryEntry> + '_ {
        self.redo.iter().rev()
    }

    /// The number of steps that can be undone.
    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// The number of steps that can be redone.
    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Pushes an entry, dropping the oldest steps beyond the depth and
    /// invalidating the redo stack.
    fn push_entry(&mut self, entry: HistoryEntry) {
        self.redo.clear();
        self.undo.push_back(entry);
        self.trim();
    }

    /// Drops the oldest steps until the undo stack fits the depth.
    fn trim(&mut self) {
        while self.undo.len() > self.depth {
            self.undo.pop_front();
        }
    }

    /// Refuses an operation that would interleave with an open group.
    fn check_no_group(&self, operation: &'static str) -> SubResult<()> {
        if self.group.is_some() {
            return Err(SubError::new(
                codes::GROUP_OPEN,
                "a command group is open: commit or abort it first",
            )
            .with_detail("operation", operation));
        }
        Ok(())
    }
}

/// Which way a history entry is being replayed.
#[derive(Debug, Clone, Copy)]
enum Direction {
    Undo,
    Redo,
}

/// Applies `command`, pairing it with the inverse it returned.
fn apply_step(project: &mut Project, command: BoxedCommand) -> SubResult<Step> {
    let inverse = command.apply_erased(project)?.into_command();
    Ok(Step {
        forward: command,
        inverse,
    })
}

/// Replays one entry in `direction`, swapping each step's forward and inverse
/// so the entry can be replayed again the other way.
///
/// Undo walks the steps backwards, redo forwards.
fn reverse(project: &mut Project, entry: &mut HistoryEntry, direction: Direction) -> SubResult<()> {
    let indices: Vec<usize> = match direction {
        Direction::Undo => (0..entry.steps.len()).rev().collect(),
        Direction::Redo => (0..entry.steps.len()).collect(),
    };
    for index in indices {
        let step = &mut entry.steps[index];
        let applied = match direction {
            Direction::Undo => &step.inverse,
            Direction::Redo => &step.forward,
        };
        let produced = applied
            .apply_erased(project)
            .map_err(|err| {
                SubError::new(
                    core_codes::INTERNAL,
                    "a command in the history could not be replayed",
                )
                .with_detail("kind", applied.kind())
                .with_detail("cause", err.to_json())
            })?
            .into_command();
        match direction {
            Direction::Undo => step.forward = produced,
            Direction::Redo => step.inverse = produced,
        }
    }
    Ok(())
}

/// Undoes `steps` in reverse order, used when a group is aborted.
fn rollback(project: &mut Project, steps: Vec<Step>) -> SubResult<()> {
    for step in steps.into_iter().rev() {
        step.inverse.apply_erased(project).map_err(|err| {
            SubError::new(
                core_codes::INTERNAL,
                "a command group could not be rolled back",
            )
            .with_detail("kind", step.inverse.kind())
            .with_detail("cause", err.to_json())
        })?;
    }
    Ok(())
}

/// Rejects a zero depth.
fn checked_depth(depth: usize) -> SubResult<usize> {
    if depth == 0 {
        return Err(SubError::new(
            core_codes::INVALID_ARGUMENT,
            "history depth must be at least one step",
        )
        .with_detail("depth", depth));
    }
    Ok(depth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_commands::{AddSequence, Failing, Rename, SetName};

    #[test]
    fn undo_and_redo_walk_the_stack() {
        let mut project = Project::new("Untitled");
        let mut history = History::new();
        assert_eq!(history.depth(), DEFAULT_DEPTH);
        assert!(!history.can_undo());
        assert!(!history.can_redo());
        assert_eq!(history.undo(&mut project).unwrap(), None);
        assert_eq!(history.redo(&mut project).unwrap(), None);

        history.apply(&mut project, SetName::new("A")).unwrap();
        history.apply(&mut project, SetName::new("B")).unwrap();
        assert_eq!(history.undo_len(), 2);
        assert_eq!(history.undo_label(), Some("test.set_name"));

        assert_eq!(
            history.undo(&mut project).unwrap().as_deref(),
            Some("test.set_name")
        );
        assert_eq!(project.name, "A");
        assert_eq!(history.redo_len(), 1);
        assert_eq!(history.redo_label(), Some("test.set_name"));

        history.undo(&mut project).unwrap();
        assert_eq!(project.name, "Untitled");
        assert!(!history.can_undo());

        history.redo(&mut project).unwrap();
        history.redo(&mut project).unwrap();
        assert_eq!(project.name, "B");
        assert!(!history.can_redo());
    }

    #[test]
    fn applying_after_an_undo_drops_the_redo_stack() {
        let mut project = Project::new("Untitled");
        let mut history = History::new();
        history.apply(&mut project, SetName::new("A")).unwrap();
        history.undo(&mut project).unwrap();
        assert!(history.can_redo());

        history.apply(&mut project, SetName::new("C")).unwrap();
        assert!(!history.can_redo());
        assert_eq!(project.name, "C");
    }

    #[test]
    fn depth_drops_the_oldest_steps() {
        let mut project = Project::new("Untitled");
        let mut history = History::with_depth(2).unwrap();
        for name in ["A", "B", "C"] {
            history.apply(&mut project, SetName::new(name)).unwrap();
        }
        assert_eq!(history.undo_len(), 2);

        // The dropped step is the one that set "A", so undoing everything
        // that is left stops at "A" rather than "Untitled".
        history.undo(&mut project).unwrap();
        history.undo(&mut project).unwrap();
        assert_eq!(project.name, "A");
        assert!(!history.can_undo());

        history.redo(&mut project).unwrap();
        history.redo(&mut project).unwrap();
        assert_eq!(project.name, "C");

        history.set_depth(1).unwrap();
        assert_eq!(history.undo_len(), 1);
        assert_eq!(history.depth(), 1);

        assert_eq!(
            History::with_depth(0).unwrap_err().code,
            core_codes::INVALID_ARGUMENT
        );
        assert_eq!(
            history.set_depth(0).unwrap_err().code,
            core_codes::INVALID_ARGUMENT
        );
    }

    #[test]
    fn a_group_undoes_as_one_step() {
        let mut project = Project::new("Untitled");
        let mut history = History::new();

        history.begin_group("Drag clips").unwrap();
        assert!(history.in_group());
        history.apply(&mut project, SetName::new("A")).unwrap();
        history
            .apply(&mut project, AddSequence::new("Main"))
            .unwrap();
        history.apply(&mut project, SetName::new("B")).unwrap();
        assert!(history.commit_group().unwrap());
        assert!(!history.in_group());

        assert_eq!(history.undo_len(), 1);
        assert_eq!(history.undo_label(), Some("Drag clips"));
        let entry = history.undo_entries().next().unwrap();
        assert_eq!(entry.len(), 3);
        assert!(!entry.is_empty());
        assert_eq!(entry.to_envelopes().unwrap().len(), 3);
        assert_eq!(
            entry.commands().map(AnyCommand::kind).collect::<Vec<_>>(),
            ["test.set_name", "test.add_sequence", "test.set_name"]
        );

        history.undo(&mut project).unwrap();
        assert_eq!(project.name, "Untitled");
        assert!(project.sequences.is_empty());

        history.redo(&mut project).unwrap();
        assert_eq!(project.name, "B");
        assert_eq!(history.redo_entries().count(), 0);
        assert_eq!(project.sequences.len(), 1);
    }

    #[test]
    fn an_empty_group_pushes_nothing() {
        let mut history = History::new();
        history.begin_group("Nothing").unwrap();
        assert!(!history.commit_group().unwrap());
        assert!(!history.can_undo());
        assert_eq!(history.commit_group().unwrap_err().code, codes::NO_GROUP);
    }

    #[test]
    fn groups_do_not_nest_and_block_undo() {
        let mut project = Project::new("Untitled");
        let mut history = History::new();
        history.begin_group("Drag").unwrap();
        assert_eq!(
            history.begin_group("Other").unwrap_err().code,
            codes::GROUP_OPEN
        );
        assert_eq!(
            history.undo(&mut project).unwrap_err().code,
            codes::GROUP_OPEN
        );
        assert_eq!(
            history.redo(&mut project).unwrap_err().code,
            codes::GROUP_OPEN
        );
        history.commit_group().unwrap();
    }

    #[test]
    fn aborting_a_group_rolls_the_project_back() {
        let mut project = Project::new("Untitled");
        let mut history = History::new();
        history.begin_group("Drag").unwrap();
        history.apply(&mut project, SetName::new("A")).unwrap();
        history
            .apply(&mut project, AddSequence::new("Main"))
            .unwrap();
        history.abort_group(&mut project).unwrap();

        assert_eq!(project.name, "Untitled");
        assert!(project.sequences.is_empty());
        assert!(!history.can_undo());
        assert!(!history.in_group());
        assert_eq!(
            history.abort_group(&mut project).unwrap_err().code,
            codes::NO_GROUP
        );
    }

    #[test]
    fn a_failing_command_leaves_no_step_and_rolls_back_its_group() {
        let mut project = Project::new("Untitled");
        let mut history = History::new();

        assert_eq!(
            history
                .apply(&mut project, Failing::new())
                .unwrap_err()
                .code,
            crate::test_commands::FAILED
        );
        assert!(!history.can_undo());

        history.begin_group("Drag").unwrap();
        history.apply(&mut project, SetName::new("A")).unwrap();
        assert_eq!(
            history
                .apply(&mut project, Failing::new())
                .unwrap_err()
                .code,
            crate::test_commands::FAILED
        );
        assert!(!history.in_group());
        assert!(!history.can_undo());
        assert_eq!(project.name, "Untitled");
    }

    #[test]
    fn clear_forgets_both_stacks() {
        let mut project = Project::new("Untitled");
        let mut history = History::new();
        history.apply(&mut project, Rename::new("A")).unwrap();
        history.apply(&mut project, Rename::new("B")).unwrap();
        assert_eq!(history.undo_label(), Some("Rename project"));
        history.undo(&mut project).unwrap();

        history.clear();
        assert!(!history.can_undo());
        assert!(!history.can_redo());
        assert_eq!(project.name, "A");
    }

    #[test]
    fn a_boxed_command_applies_like_any_other() {
        let mut project = Project::new("Untitled");
        let mut history = History::default();
        history
            .apply_boxed(&mut project, Box::new(SetName::new("A")))
            .unwrap();
        assert_eq!(project.name, "A");
        assert_eq!(history.undo_len(), 1);
    }
}
