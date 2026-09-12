//! The editor's engine handle: the one place the UI mutates a project.
//!
//! Every panel in this crate plans its gesture and hands the plan back; this
//! is what applies it. The project itself lives on the engine thread
//! (`sub_edit::Engine`, docs/PLAN.md §4), so the UI never owns a mutable
//! [`Project`] at all: it reads an immutable snapshot each frame and sends
//! commands the other way. That is what makes an edit made through the MCP
//! bridge and an edit made with the mouse the same edit — both arrive as
//! commands on the same queue, and both come back on the same change-event
//! subscription.
//!
//! - [`EditorSession::poll`] drains that subscription once a frame and
//!   refreshes the snapshot when the revision moved, which is how an outside
//!   edit reaches the panels.
//! - [`EditorSession::apply_group`] is the one entry point for a gesture that
//!   is more than one command: the whole drag becomes one entry in the undo
//!   stack.
//! - Opening a project starts a fresh engine. The engine owns the project for
//!   its whole life, so replacing the project means replacing the engine, and
//!   the autosave worker and the event subscription are rebuilt with it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sub_command::Dispatcher;
use sub_core::{SubError, SubResult};
use sub_edit::{
    Autosave, AutosaveConfig, AutosaveStatus, BoxedCommand, ChangeEvent, Command, Engine,
    EngineHandle, EventReceiver, HistorySummary, SnapshotStore,
};
use sub_model::{Project, SequenceId};

/// The engine, the project it owns, and the workers hanging off it.
pub struct EditorSession {
    /// The engine thread. Dropping it stops the thread.
    engine: Engine,
    /// The last snapshot read from the engine. Immutable, so a panel may hold
    /// it for a whole frame without blocking any writer.
    project: Arc<Project>,
    /// The revision `project` is a snapshot of.
    revision: u64,
    /// The change-event subscription this session reads once a frame.
    events: EventReceiver<ChangeEvent>,
    /// The Command API over that engine, in process. It is the same surface
    /// the MCP bridge and the CLI reach over the socket, so a window feature
    /// that cannot be expressed as a Command API call does not exist
    /// (docs/PLAN.md §4). Shared rather than owned, because the socket server
    /// ([`crate::command_api`]) serves this very dispatcher from its own
    /// threads.
    commands: Arc<Dispatcher>,
    /// How many engines this session has had. Opening a project replaces the
    /// engine, and with it the dispatcher; the socket server watches this so
    /// it can rebind onto the engine the panels are now drawing.
    generation: u64,
    /// The file the project came from, once one has been opened.
    project_file: Option<PathBuf>,
    /// The autosave worker, running whenever a project file is known.
    autosave: Option<Autosave>,
    /// What the last command, save or open reported, for the status bar.
    last_error: Option<SubError>,
}

impl EditorSession {
    /// Starts an engine owning `project`.
    ///
    /// # Errors
    ///
    /// Whatever [`Engine::spawn`] returns.
    pub fn new(project: Project) -> SubResult<Self> {
        let engine = Engine::spawn(project)?;
        let events = engine.handle().subscribe();
        let project = engine.handle().snapshot();
        let revision = engine.handle().revision();
        let commands = Arc::new(Dispatcher::new(engine.handle().clone()));
        Ok(Self {
            engine,
            project,
            revision,
            events,
            commands,
            generation: 0,
            project_file: None,
            autosave: None,
            last_error: None,
        })
    }

    /// The handle the Command API, the MCP bridge and the panels all share.
    #[must_use]
    pub fn handle(&self) -> &EngineHandle {
        self.engine.handle()
    }

    /// The Command API over this engine, in process.
    ///
    /// Every method the MCP bridge and the CLI can call is here, running
    /// against the very project the panels are drawing.
    #[must_use]
    pub fn commands(&self) -> &Dispatcher {
        &self.commands
    }

    /// The same dispatcher as a shared pointer, for the socket server that
    /// serves it to other processes.
    #[must_use]
    pub fn commands_arc(&self) -> Arc<Dispatcher> {
        Arc::clone(&self.commands)
    }

    /// How many engines this session has had, counting from zero.
    ///
    /// It moves when — and only when — the project is replaced, which is the
    /// one event that invalidates a dispatcher handed out earlier.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The project as of the last poll.
    #[must_use]
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The same snapshot as a shared pointer, for a worker that outlives the
    /// frame.
    #[must_use]
    pub fn project_arc(&self) -> Arc<Project> {
        Arc::clone(&self.project)
    }

    /// The revision the snapshot belongs to.
    ///
    /// The timeline keys its cached layout off this, so a change made
    /// anywhere — a panel, the Command API, a plugin — invalidates it.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// The file the project came from, once one has been opened.
    #[must_use]
    pub fn project_file(&self) -> Option<&Path> {
        self.project_file.as_deref()
    }

    /// What the last operation reported, if it failed.
    #[must_use]
    pub const fn last_error(&self) -> Option<&SubError> {
        self.last_error.as_ref()
    }

    /// Forgets the last error, once it has been shown.
    pub fn clear_error(&mut self) {
        self.last_error = None;
    }

    /// What the autosave worker has done so far, when one is running.
    #[must_use]
    pub fn autosave_status(&self) -> Option<AutosaveStatus> {
        self.autosave.as_ref().map(Autosave::status)
    }

    /// Reads the change events published since the last call and refreshes the
    /// snapshot when the project moved on.
    ///
    /// Returns whether the snapshot changed, which is the panels' cue to
    /// repaint. This is what makes an edit applied through the Command API
    /// appear in the running UI: the engine broadcasts it, and the next frame
    /// reads it here.
    pub fn poll(&mut self) -> bool {
        while self.events.try_recv().is_some() {}
        // A lagged subscriber missed events but not the revision counter, so
        // the comparison below still catches everything it dropped.
        let _ = self.events.take_lagged();
        if self.handle().revision() == self.revision {
            return false;
        }
        self.refresh();
        true
    }

    /// Re-reads the snapshot and the revision from the engine.
    fn refresh(&mut self) {
        self.project = self.handle().snapshot();
        self.revision = self.handle().revision();
    }

    /// Applies one command and refreshes the snapshot.
    ///
    /// # Errors
    ///
    /// Whatever the command returns, or `edit.engine_stopped`.
    pub fn apply<C: Command>(&mut self, command: C) -> SubResult<()> {
        self.apply_boxed(Box::new(command))
    }

    /// Applies one already-boxed command and refreshes the snapshot.
    ///
    /// # Errors
    ///
    /// Whatever the command returns, or `edit.engine_stopped`.
    pub fn apply_boxed(&mut self, command: BoxedCommand) -> SubResult<()> {
        let result = self.handle().apply_boxed(command);
        self.refresh();
        match result {
            Ok(_) => Ok(()),
            Err(error) => {
                self.remember(&error);
                Err(error)
            }
        }
    }

    /// Applies `commands` as one entry in the undo stack labelled `label`.
    ///
    /// This is how a drag that moves five clips is undone with one press: the
    /// group is opened on the engine, every command goes inside it, and it is
    /// committed. A command that fails aborts the group, so the project is
    /// left exactly as it was rather than half edited.
    ///
    /// # Errors
    ///
    /// `edit.group_open` when another client already has a group open,
    /// whatever a command returns, or `edit.engine_stopped`.
    pub fn apply_group(
        &mut self,
        label: impl Into<String>,
        commands: Vec<BoxedCommand>,
    ) -> SubResult<()> {
        if commands.is_empty() {
            return Ok(());
        }
        let result = self.grouped(label.into(), commands);
        self.refresh();
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                self.remember(&error);
                Err(error)
            }
        }
    }

    /// The body of [`EditorSession::apply_group`], so every exit refreshes.
    fn grouped(&self, label: String, commands: Vec<BoxedCommand>) -> SubResult<()> {
        self.handle().begin_group(label)?;
        for command in commands {
            if let Err(error) = self.handle().apply_boxed(command) {
                // The group is abandoned rather than committed: half a drag is
                // not an edit anyone asked for.
                let _ = self.handle().abort_group();
                return Err(error);
            }
        }
        self.handle().commit_group()?;
        Ok(())
    }

    /// Opens a history group that later frames add to, for a gesture that
    /// spans several frames (a gain drag, an inspector slider).
    ///
    /// # Errors
    ///
    /// `edit.group_open`, or `edit.engine_stopped`.
    pub fn begin_group(&mut self, label: impl Into<String>) -> SubResult<()> {
        match self.handle().begin_group(label) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.remember(&error);
                Err(error)
            }
        }
    }

    /// Closes the open group as one undo step.
    ///
    /// # Errors
    ///
    /// `edit.no_group`, or `edit.engine_stopped`.
    pub fn commit_group(&mut self) -> SubResult<()> {
        let result = self.handle().commit_group();
        self.refresh();
        match result {
            Ok(_) => Ok(()),
            Err(error) => {
                self.remember(&error);
                Err(error)
            }
        }
    }

    /// Undoes the most recent step. Returns whether anything was undone.
    ///
    /// # Errors
    ///
    /// `edit.group_open` while a gesture is open, or `edit.engine_stopped`.
    pub fn undo(&mut self) -> SubResult<bool> {
        let result = self.handle().undo();
        self.refresh();
        match result {
            Ok(applied) => Ok(applied.is_some()),
            Err(error) => {
                self.remember(&error);
                Err(error)
            }
        }
    }

    /// Redoes the most recently undone step. Returns whether anything moved.
    ///
    /// # Errors
    ///
    /// The same as [`EditorSession::undo`].
    pub fn redo(&mut self) -> SubResult<bool> {
        let result = self.handle().redo();
        self.refresh();
        match result {
            Ok(applied) => Ok(applied.is_some()),
            Err(error) => {
                self.remember(&error);
                Err(error)
            }
        }
    }

    /// The state of the undo and redo stacks, as the Edit menu reads it.
    ///
    /// `None` once the engine thread is gone; the menu is drawn every frame
    /// and has nowhere to put an error.
    #[must_use]
    pub fn history(&self) -> Option<HistorySummary> {
        self.handle().history().ok()
    }

    /// Replaces the project with `project`, starting a fresh engine for it.
    ///
    /// The engine owns the project for its whole life, so a new project means
    /// a new engine: the undo stack of the project being closed goes with it,
    /// which is what an editor does when a file is closed.
    ///
    /// # Errors
    ///
    /// Whatever [`Engine::spawn`] returns.
    pub fn adopt(&mut self, project: Project, file: Option<PathBuf>) -> SubResult<()> {
        // The autosave worker holds a handle to the outgoing engine; it is
        // stopped first so its last snapshot belongs to the project it was
        // watching.
        self.autosave = None;
        let engine = Engine::spawn(project)?;
        self.events = engine.handle().subscribe();
        self.project = engine.handle().snapshot();
        self.revision = engine.handle().revision();
        // The dispatcher holds a handle to the outgoing engine, so it is
        // rebuilt with the new one rather than left pointing at a dead thread.
        self.commands = Arc::new(Dispatcher::new(engine.handle().clone()));
        self.generation += 1;
        self.engine = engine;
        self.project_file = file;
        self.start_autosave();
        Ok(())
    }

    /// Reads a project file and adopts it.
    ///
    /// # Errors
    ///
    /// - `ui.project_unreadable` when the file cannot be read.
    /// - Whatever `sub_model::json::from_json` returns for its contents.
    /// - Whatever [`Engine::spawn`] returns.
    pub fn open(&mut self, path: &Path) -> SubResult<()> {
        match Self::read(path).and_then(|project| self.adopt(project, Some(path.to_owned()))) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.remember(&error);
                Err(error)
            }
        }
    }

    /// Reads a project file without touching the session.
    ///
    /// # Errors
    ///
    /// `ui.project_unreadable`, or whatever the loader returns.
    pub fn read(path: &Path) -> SubResult<Project> {
        let text = std::fs::read_to_string(path).map_err(|err| {
            SubError::wrap(
                crate::codes::PROJECT_UNREADABLE,
                "a project cannot be read",
                &err,
            )
            .with_detail("path", path.display().to_string())
        })?;
        sub_model::json::from_json(&text)
    }

    /// Writes the project back to the file it came from.
    ///
    /// # Errors
    ///
    /// `ui.project_unsaved` when no file is known or the write fails, or
    /// whatever the serialiser returns.
    pub fn save(&mut self) -> SubResult<()> {
        let Some(path) = self.project_file.clone() else {
            let error = SubError::new(
                crate::codes::PROJECT_UNSAVED,
                "this project has never been saved; choose a file first",
            );
            self.remember(&error);
            return Err(error);
        };
        self.save_as(&path)
    }

    /// Writes the project to `path` and remembers it as the project's file.
    ///
    /// # Errors
    ///
    /// `ui.project_unsaved` when the write fails, or whatever the serialiser
    /// returns.
    pub fn save_as(&mut self, path: &Path) -> SubResult<()> {
        match self.write_to(path) {
            Ok(()) => {
                let fresh = self.project_file.as_deref() != Some(path);
                self.project_file = Some(path.to_owned());
                if fresh {
                    self.start_autosave();
                }
                Ok(())
            }
            Err(error) => {
                self.remember(&error);
                Err(error)
            }
        }
    }

    /// Serialises the current snapshot to `path`.
    fn write_to(&self, path: &Path) -> SubResult<()> {
        let text = sub_model::json::to_json(&self.project)?;
        std::fs::write(path, text).map_err(|err| {
            SubError::wrap(
                crate::codes::PROJECT_UNSAVED,
                "a project cannot be written",
                &err,
            )
            .with_detail("path", path.display().to_string())
        })
    }

    /// Starts the autosave worker for the open project file, replacing any
    /// worker already running.
    ///
    /// A sidecar directory that cannot be made costs the user their autosave
    /// history, never their session, so the failure is remembered and the
    /// editor carries on.
    pub fn start_autosave(&mut self) {
        self.autosave = None;
        let Some(path) = self.project_file.clone() else {
            return;
        };
        match SnapshotStore::for_project(&path)
            .and_then(|store| Autosave::spawn(self.handle(), store, AutosaveConfig::default()))
        {
            Ok(worker) => self.autosave = Some(worker),
            Err(error) => self.remember(&error),
        }
    }

    /// Stops the autosave worker, if one is running.
    pub fn stop_autosave(&mut self) {
        self.autosave = None;
    }

    /// The first sequence of the project, which is the one a fresh tab strip
    /// shows.
    #[must_use]
    pub fn first_sequence(&self) -> Option<SequenceId> {
        self.project.sequences.first().map(|sequence| sequence.id)
    }

    /// Records `error` for the status bar and the log.
    fn remember(&mut self, error: &SubError) {
        log::warn!("[{}] {}", error.code, error.message);
        self.last_error = Some(error.clone());
    }
}

impl std::fmt::Debug for EditorSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EditorSession")
            .field("revision", &self.revision)
            .field("generation", &self.generation)
            .field("project_file", &self.project_file)
            .field("autosaving", &self.autosave.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::EditorSession;
    use std::path::PathBuf;
    use sub_edit::commands::RenameSequence;
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Project, Sequence, SequenceId};

    /// A folder of this test's own, emptied first so a rerun starts clean.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sub-ui-session-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        dir
    }

    /// A project with one sequence, so a command has something to name.
    fn project() -> Project {
        let mut project = Project::new("Doc cut");
        project
            .sequences
            .push(Sequence::new("Main", SequenceSettings::default()));
        project
    }

    #[test]
    fn a_command_moves_the_snapshot_and_the_revision() {
        let mut session = EditorSession::new(project()).expect("the engine starts");
        assert_eq!(session.revision(), 0);
        let sequence = session.first_sequence().expect("a sequence");

        session
            .apply(RenameSequence::new(sequence, "Rough cut"))
            .expect("the rename applies");
        assert_eq!(session.revision(), 1);
        assert_eq!(session.project().sequences[0].name, "Rough cut");

        assert!(session.undo().expect("the undo runs"), "a step was undone");
        assert_eq!(session.project().sequences[0].name, "Main");
    }

    #[test]
    fn a_failed_group_leaves_the_project_alone() {
        let mut session = EditorSession::new(project()).expect("the engine starts");
        let sequence = session.first_sequence().expect("a sequence");
        let missing = SequenceId::new();

        let error = session
            .apply_group(
                "Two renames",
                vec![
                    Box::new(RenameSequence::new(sequence, "First")),
                    Box::new(RenameSequence::new(missing, "Never")),
                ],
            )
            .expect_err("the second command names no sequence");
        assert_eq!(error.code.as_str(), "edit.sequence_not_found");
        assert_eq!(
            session.project().sequences[0].name,
            "Main",
            "the group was abandoned rather than half applied"
        );
        assert!(
            !session.history().expect("a history").can_undo,
            "an abandoned group leaves nothing on the undo stack"
        );
    }

    #[test]
    fn save_as_writes_the_project_and_starts_autosaving_it() {
        let dir = temp_dir("save");
        let path = dir.join("cut.sub");
        let mut session = EditorSession::new(project()).expect("the engine starts");
        let sequence = session.first_sequence().expect("a sequence");
        session
            .apply(RenameSequence::new(sequence, "Rough cut"))
            .expect("the rename applies");

        assert!(
            session.save().is_err(),
            "a project with no file cannot be saved without choosing one"
        );
        assert_eq!(
            session
                .last_error()
                .expect("the refusal is remembered")
                .code
                .as_str(),
            "ui.project_unsaved"
        );

        session.save_as(&path).expect("the project writes");
        assert_eq!(session.project_file(), Some(path.as_path()));
        let written = EditorSession::read(&path).expect("the file reads back");
        assert_eq!(written.sequences[0].name, "Rough cut");

        // A file is now known, so the autosave worker is watching this engine.
        let status = session.autosave_status().expect("an autosave worker");
        assert!(status.last_error.is_none(), "{:?}", status.last_error);

        // Saving again goes to the same file, with no second choice needed.
        session
            .apply(RenameSequence::new(sequence, "Fine cut"))
            .expect("the rename applies");
        session.save().expect("the project writes again");
        let again = EditorSession::read(&path).expect("the file reads back");
        assert_eq!(again.sequences[0].name, "Fine cut");
    }

    #[test]
    fn opening_a_file_replaces_the_engine_and_its_history() {
        let dir = temp_dir("open");
        let path = dir.join("cut.sub");
        let text = sub_model::json::to_json(&project()).expect("the project serialises");
        std::fs::write(&path, text).expect("the fixture writes");

        let mut session = EditorSession::new(Project::new("Untitled")).expect("the engine starts");
        assert_eq!(
            session.first_sequence(),
            None,
            "an untitled project has no sequences"
        );

        session.open(&path).expect("the project opens");
        assert_eq!(session.project().sequences.len(), 1);
        assert_eq!(session.project_file(), Some(path.as_path()));
        assert_eq!(
            session.revision(),
            0,
            "opening a file is not an edit; the new engine starts at zero"
        );
        assert!(
            !session.history().expect("a history").can_undo,
            "the closed project's undo stack went with its engine"
        );
    }

    #[test]
    fn the_command_api_and_the_panels_read_one_project() {
        let mut session = EditorSession::new(project()).expect("the engine starts");
        let sequence = session.first_sequence().expect("a sequence");
        session
            .apply(RenameSequence::new(sequence, "Rough cut"))
            .expect("the rename applies");

        let value = session
            .commands()
            .invoke("project.revision", None)
            .expect("project.revision answers");
        assert_eq!(value["revision"], 1);
    }

    #[test]
    fn a_change_from_outside_the_window_reaches_the_next_poll() {
        let mut session = EditorSession::new(project()).expect("the engine starts");
        let sequence = session.first_sequence().expect("a sequence");
        assert!(!session.poll(), "nothing has happened yet");

        // The path the MCP bridge takes: a command straight on the handle.
        session
            .handle()
            .apply(RenameSequence::new(sequence, "Renamed by an agent"))
            .expect("the rename applies");
        assert!(session.poll(), "the poll saw the change");
        assert_eq!(session.project().sequences[0].name, "Renamed by an agent");
        assert!(!session.poll(), "a second poll has nothing new to report");
    }
}
