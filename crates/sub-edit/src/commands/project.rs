//! The project command: swapping the whole open document.
//!
//! Opening a file and starting a new document are the two edits that replace
//! everything at once, and they are edits like any other: [`ReplaceProject`]
//! goes through the history, so an agent that opened the wrong file undoes it
//! (decision-7). The inverse carries the outgoing project whole, which is what
//! makes the undo exact down to the identifiers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::SubResult;
use sub_model::Project;

use crate::{Command, Inverse};

/// Replaces the whole open project.
///
/// ```
/// use sub_edit::History;
/// use sub_edit::commands::ReplaceProject;
/// use sub_model::Project;
///
/// let mut project = Project::new("Doc cut");
/// let mut history = History::new();
/// history
///     .apply(&mut project, ReplaceProject::new(Project::new("Trailer")))
///     .unwrap();
/// assert_eq!(project.name, "Trailer");
///
/// history.undo(&mut project).unwrap();
/// assert_eq!(project.name, "Doc cut");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceProject {
    /// The document that takes the open one's place.
    pub project: Box<Project>,
}

impl ReplaceProject {
    /// Replaces the open project with `project`.
    #[must_use]
    pub fn new(project: Project) -> Self {
        Self {
            project: Box::new(project),
        }
    }
}

impl Command for ReplaceProject {
    const KIND: &'static str = "project.replace";
    const DESCRIPTION: &'static str = "Replace the whole open project with another one.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let previous = std::mem::replace(project, (*self.project).clone());
        Ok(Inverse::new(Self::new(previous)))
    }

    fn label(&self) -> String {
        format!("Open {}", self.project.name)
    }
}

#[cfg(test)]
mod tests {
    use super::ReplaceProject;
    use crate::{Command as _, History};
    use sub_model::{Project, Sequence, SequenceSettings};

    #[test]
    fn replacing_a_project_undoes_back_to_the_one_before_it() {
        let mut project = Project::new("Doc cut");
        let before = project.clone();
        let mut incoming = Project::new("Trailer");
        incoming
            .sequences
            .push(Sequence::new("Main", SequenceSettings::default()));

        let mut history = History::new();
        history
            .apply(&mut project, ReplaceProject::new(incoming.clone()))
            .expect("the project is replaced");
        assert_eq!(project, incoming);

        history.undo(&mut project).expect("the swap undoes");
        assert_eq!(project, before);
        history.redo(&mut project).expect("the swap redoes");
        assert_eq!(project, incoming);
    }

    #[test]
    fn the_label_names_the_incoming_project() {
        let command = ReplaceProject::new(Project::new("Trailer"));
        assert_eq!(command.label(), "Open Trailer");
    }
}
