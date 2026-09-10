//! The autosave recovery prompt and the snapshot restore menu.
//!
//! `sub_edit::autosave` keeps a history of complete project snapshots in the
//! project's sidecar directory and answers two questions about it. This module
//! is the pair of widgets that ask a user those questions:
//!
//! - [`RecoveryPrompt`] is what an open shows when the newest autosave is
//!   newer than the project file, which is the signature of a session that
//!   ended without saving. It states what would be lost and offers the two
//!   answers, "Recover" and "Discard".
//! - [`SnapshotMenu`] is the restore list: the last few autosaves, newest
//!   first, each one restorable with a click.
//!
//! Neither widget mutates the open project. Restoring a snapshot is not a
//! command, because a snapshot is a whole project rather than an edit to one:
//! both widgets hand a loaded [`Project`] back and the caller opens it, with a
//! fresh engine and a fresh history, exactly as opening a file does.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use eframe::egui;
use sub_core::{SubError, SubResult};
use sub_edit::autosave::{Recovery, Snapshot, SnapshotStore};
use sub_model::Project;

/// The prompt's window title.
pub const PROMPT_TITLE: &str = "Recover unsaved work";

/// The button that opens the newest autosave.
pub const RECOVER_LABEL: &str = "Recover";

/// The button that throws the autosave history away.
pub const DISCARD_LABEL: &str = "Discard";

/// The restore menu's title.
pub const MENU_TITLE: &str = "Snapshot history";

/// What the restore menu says when there is nothing to restore.
pub const EMPTY_LABEL: &str = "No autosaves yet";

/// What the menu says before a project file has been opened.
pub const NO_PROJECT_LABEL: &str = "Open a project to see its autosaves";

/// What a prompt or a restore click produced.
///
/// A failed load is carried rather than raised: an unreadable snapshot is a
/// reason to tell the user and leave the project they have open alone, never a
/// reason to end the session.
#[derive(Debug)]
pub enum RecoveryOutcome {
    /// A project was read back and is the one to open. Boxed because a
    /// `Project` is far larger than the other answers.
    Recovered(Box<Project>),
    /// The autosave history was thrown away and the project file kept.
    Discarded,
    /// Neither happened, for this reason.
    Failed(SubError),
}

impl RecoveryOutcome {
    /// The project this outcome recovered, if it recovered one.
    #[must_use]
    pub fn project(self) -> Option<Project> {
        match self {
            Self::Recovered(project) => Some(*project),
            _ => None,
        }
    }

    /// Why nothing happened, when nothing did.
    #[must_use]
    pub fn error(&self) -> Option<&SubError> {
        match self {
            Self::Failed(error) => Some(error),
            _ => None,
        }
    }
}

/// The dialog an open shows when an autosave is ahead of the project file.
///
/// It is closed until [`RecoveryPrompt::open_for`] finds something to ask
/// about, and it closes itself again as soon as it is answered.
#[derive(Debug, Default)]
pub struct RecoveryPrompt {
    recovery: Option<Recovery>,
    /// The last failure, kept on screen until the prompt is answered again.
    error: Option<SubError>,
}

impl RecoveryPrompt {
    /// A closed prompt.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks whether opening `project_file` should prompt, and opens the prompt
    /// when it should.
    ///
    /// Returns whether the prompt is now open, so a caller that opens a
    /// project can tell the two paths apart.
    ///
    /// # Errors
    ///
    /// Whatever [`sub_edit::autosave::check_for_recovery`] returns: the path
    /// names no file, or the sidecar directory cannot be read.
    pub fn open_for(&mut self, project_file: &Path) -> SubResult<bool> {
        let found = sub_edit::autosave::check_for_recovery(project_file)?;
        self.open(found);
        Ok(self.is_open())
    }

    /// Opens the prompt on a recovery that has already been found, or closes
    /// it when there is none.
    pub fn open(&mut self, recovery: Option<Recovery>) {
        self.error = None;
        self.recovery = recovery;
    }

    /// Whether the prompt is on screen.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.recovery.is_some()
    }

    /// The recovery being offered, while one is.
    #[must_use]
    pub fn recovery(&self) -> Option<&Recovery> {
        self.recovery.as_ref()
    }

    /// The question, as the prompt states it.
    #[must_use]
    pub fn label(&self) -> Option<String> {
        self.recovery.as_ref().map(Recovery::label)
    }

    /// The last failure, until the prompt is opened again.
    #[must_use]
    pub fn error(&self) -> Option<&SubError> {
        self.error.as_ref()
    }

    /// Closes the prompt without answering it.
    pub fn close(&mut self) {
        self.recovery = None;
    }

    /// Answers "recover": loads the autosaved project and closes the prompt.
    ///
    /// The snapshot stays on disk; nothing is destroyed until the recovered
    /// project is saved over the file.
    pub fn recover(&mut self) -> Option<RecoveryOutcome> {
        let recovery = self.recovery.take()?;
        Some(
            self.record(
                recovery
                    .recover()
                    .map(Box::new)
                    .map(RecoveryOutcome::Recovered),
            ),
        )
    }

    /// Answers "discard": drops the autosave history so the next open does not
    /// ask again, and leaves the project file exactly as it is.
    pub fn discard(&mut self) -> Option<RecoveryOutcome> {
        let recovery = self.recovery.take()?;
        Some(self.record(recovery.discard().map(|_| RecoveryOutcome::Discarded)))
    }

    /// Turns a failed answer into [`RecoveryOutcome::Failed`] and keeps the
    /// reason on screen.
    fn record(&mut self, result: SubResult<RecoveryOutcome>) -> RecoveryOutcome {
        match result {
            Ok(outcome) => {
                self.error = None;
                outcome
            }
            Err(error) => {
                self.error = Some(error.clone());
                RecoveryOutcome::Failed(error)
            }
        }
    }

    /// Draws the prompt, if it is open, and answers it when a button is
    /// pressed.
    ///
    /// Returns `None` on every frame the user does not answer.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<RecoveryOutcome> {
        if !self.is_open() {
            return None;
        }
        let mut answer = None;
        egui::Window::new(PROMPT_TITLE)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                answer = self.body_ui(ui);
            });
        answer
    }

    /// The prompt's contents, drawn into whatever container the caller has.
    ///
    /// Split out from [`RecoveryPrompt::ui`] so a test can lay the prompt out
    /// on its own, without a window around it.
    pub fn body_ui(&mut self, ui: &mut egui::Ui) -> Option<RecoveryOutcome> {
        let recovery = self.recovery.as_ref()?;
        ui.label(recovery.label());
        ui.label(format!(
            "The autosave is {}.",
            age_text(recovery.snapshot().saved_at(), SystemTime::now())
        ));
        if let Some(ahead) = recovery.ahead_by() {
            ui.label(format!(
                "Opening the file as it is would lose {} of work.",
                duration_text(ahead)
            ));
        }
        if let Some(error) = self.error.as_ref() {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("[{}] {}", error.code, error.message),
            );
        }
        let mut answer = None;
        ui.horizontal(|ui| {
            if ui.button(RECOVER_LABEL).clicked() {
                answer = self.recover();
            }
            if ui.button(DISCARD_LABEL).clicked() {
                answer = self.discard();
            }
        });
        answer
    }
}

/// The restore list: one project's autosaves, newest first.
///
/// The list is read from disk once, when a project is set and whenever
/// [`SnapshotMenu::refresh`] is called, rather than on every frame, because a
/// menu is drawn many times a second and the history changes only when the
/// autosave worker writes.
#[derive(Debug, Default)]
pub struct SnapshotMenu {
    project_file: Option<PathBuf>,
    store: Option<SnapshotStore>,
    snapshots: Vec<Snapshot>,
    error: Option<SubError>,
}

impl SnapshotMenu {
    /// An empty menu, belonging to no project yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Points the menu at a project file and reads its history.
    ///
    /// # Errors
    ///
    /// - `core.invalid_argument` when the path names no file.
    /// - `edit.autosave_failed` when the sidecar directory cannot be listed.
    pub fn set_project(&mut self, project_file: &Path) -> SubResult<()> {
        self.project_file = Some(project_file.to_path_buf());
        self.store = Some(SnapshotStore::for_project(project_file)?);
        self.refresh()
    }

    /// Forgets the project, leaving the menu empty.
    pub fn clear(&mut self) {
        self.project_file = None;
        self.store = None;
        self.snapshots.clear();
        self.error = None;
    }

    /// Re-reads the history from the sidecar directory.
    ///
    /// # Errors
    ///
    /// Returns `edit.autosave_failed` when the directory cannot be listed.
    pub fn refresh(&mut self) -> SubResult<()> {
        let Some(store) = self.store.as_ref() else {
            self.snapshots.clear();
            return Ok(());
        };
        match store.list() {
            Ok(snapshots) => {
                self.snapshots = snapshots;
                self.error = None;
                Ok(())
            }
            Err(error) => {
                self.snapshots.clear();
                self.error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// The project file whose history is listed, once one is set.
    #[must_use]
    pub fn project_file(&self) -> Option<&Path> {
        self.project_file.as_deref()
    }

    /// The snapshots on offer, newest first.
    #[must_use]
    pub fn snapshots(&self) -> &[Snapshot] {
        &self.snapshots
    }

    /// The last failure to read the history, until the next refresh.
    #[must_use]
    pub fn error(&self) -> Option<&SubError> {
        self.error.as_ref()
    }

    /// The menu entries, as they are drawn, newest first.
    #[must_use]
    pub fn entry_labels(&self) -> Vec<String> {
        let now = SystemTime::now();
        self.snapshots
            .iter()
            .map(|snapshot| entry_label(snapshot, now))
            .collect()
    }

    /// Loads the snapshot at `index` in the list.
    ///
    /// # Errors
    ///
    /// - `edit.snapshot_not_found` when the entry names nothing, either
    ///   because the index is past the end or because the file has since been
    ///   pruned.
    /// - Whatever [`Snapshot::load`] returns for a file it cannot read or
    ///   parse.
    pub fn restore(&self, index: usize) -> SubResult<Project> {
        let snapshot = self.snapshots.get(index).ok_or_else(|| {
            SubError::new(
                sub_edit::codes::SNAPSHOT_NOT_FOUND,
                "no autosave snapshot at that position",
            )
            .with_detail("index", index.to_string())
        })?;
        snapshot.load()
    }

    /// Draws the list and restores whichever entry is clicked.
    ///
    /// Meant for the body of a menu button, so it is `&mut egui::Ui` rather
    /// than a context: the caller decides where the list hangs.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<RecoveryOutcome> {
        if self.store.is_none() {
            ui.label(NO_PROJECT_LABEL);
            return None;
        }
        if let Some(error) = self.error.as_ref() {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("[{}] {}", error.code, error.message),
            );
            return None;
        }
        if self.snapshots.is_empty() {
            ui.label(EMPTY_LABEL);
            return None;
        }
        let now = SystemTime::now();
        let mut chosen = None;
        for (index, snapshot) in self.snapshots.iter().enumerate() {
            if ui.button(entry_label(snapshot, now)).clicked() {
                chosen = Some(index);
                ui.close();
            }
        }
        let index = chosen?;
        Some(match self.restore(index) {
            Ok(project) => RecoveryOutcome::Recovered(Box::new(project)),
            Err(error) => {
                self.error = Some(error.clone());
                RecoveryOutcome::Failed(error)
            }
        })
    }
}

/// How one snapshot reads in the restore list.
///
/// The engine revision says which is which when two autosaves are a second
/// apart, and the age is what a user actually recognises.
#[must_use]
pub fn entry_label(snapshot: &Snapshot, now: SystemTime) -> String {
    format!(
        "{} ({})",
        snapshot.label(),
        age_text(snapshot.saved_at(), now)
    )
}

/// "just now", "3 minutes ago", "2 hours ago": how old `then` is at `now`.
///
/// A snapshot stamped in the future — a clock that was put back — reads as
/// "just now" rather than as a negative age.
#[must_use]
pub fn age_text(then: SystemTime, now: SystemTime) -> String {
    let Ok(elapsed) = now.duration_since(then) else {
        return "just now".to_owned();
    };
    if elapsed.as_secs() < 10 {
        return "just now".to_owned();
    }
    format!("{} ago", duration_text(elapsed))
}

/// A whole-unit description of `span`, in the largest unit that fits.
///
/// Every step is integer arithmetic: this crate rounds times for a label, and
/// timeline arithmetic never comes near it.
#[must_use]
pub fn duration_text(span: Duration) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    let seconds = span.as_secs();
    let (count, unit) = match seconds {
        0..MINUTE => (seconds.max(1), "second"),
        MINUTE..HOUR => (seconds / MINUTE, "minute"),
        HOUR..DAY => (seconds / HOUR, "hour"),
        _ => (seconds / DAY, "day"),
    };
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EMPTY_LABEL, NO_PROJECT_LABEL, RecoveryOutcome, RecoveryPrompt, SnapshotMenu, age_text,
        duration_text, entry_label,
    };
    use std::time::{Duration, SystemTime};
    use sub_edit::autosave::SnapshotStore;
    use sub_model::Project;

    /// A directory of this test's own, emptied first so a rerun starts clean.
    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sub-ui-recovery-unit-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the temporary directory is creatable");
        dir
    }

    #[test]
    fn a_span_reads_in_the_largest_whole_unit_that_fits() {
        assert_eq!(duration_text(Duration::from_secs(0)), "1 second");
        assert_eq!(duration_text(Duration::from_secs(1)), "1 second");
        assert_eq!(duration_text(Duration::from_secs(45)), "45 seconds");
        assert_eq!(duration_text(Duration::from_mins(1)), "1 minute");
        assert_eq!(duration_text(Duration::from_secs(200)), "3 minutes");
        assert_eq!(duration_text(Duration::from_hours(1)), "1 hour");
        assert_eq!(duration_text(Duration::from_hours(5)), "5 hours");
        assert_eq!(duration_text(Duration::from_hours(48)), "2 days");
    }

    #[test]
    fn a_recent_or_future_snapshot_reads_as_just_now() {
        let now = SystemTime::now();
        assert_eq!(age_text(now, now), "just now");
        assert_eq!(age_text(now + Duration::from_mins(1), now), "just now");
        assert_eq!(age_text(now - Duration::from_secs(5), now), "just now");
        assert_eq!(age_text(now - Duration::from_mins(5), now), "5 minutes ago");
    }

    #[test]
    fn an_entry_names_the_revision_and_the_age() {
        let dir = temp_dir("entry");
        let store = SnapshotStore::new(dir.join("autosave"));
        let snapshot = store
            .write(&Project::new("Doc cut"), 7, 4)
            .expect("the snapshot is written");
        let label = entry_label(&snapshot, SystemTime::now() + Duration::from_mins(2));
        assert!(
            label.contains("revision 7"),
            "it names the revision: {label}"
        );
        assert!(label.contains("2 minutes ago"), "and the age: {label}");
    }

    #[test]
    fn a_menu_with_no_project_says_so_and_restores_nothing() {
        let mut menu = SnapshotMenu::new();
        assert!(menu.project_file().is_none());
        assert!(menu.snapshots().is_empty());
        assert!(menu.entry_labels().is_empty());
        assert!(menu.refresh().is_ok(), "an empty menu refreshes to nothing");
        let error = menu.restore(0).expect_err("there is nothing at index 0");
        assert_eq!(error.code, sub_edit::codes::SNAPSHOT_NOT_FOUND);
        // The two things the menu body can say without a list.
        assert_ne!(NO_PROJECT_LABEL, EMPTY_LABEL);
    }

    #[test]
    fn opening_a_project_asks_the_autosave_history_and_prompts_when_it_is_ahead() {
        let dir = temp_dir("open-for");
        let project_file = dir.join("doc-cut.sub");

        // Nothing autosaved: an open goes straight through.
        let mut prompt = RecoveryPrompt::new();
        assert!(
            !prompt
                .open_for(&project_file)
                .expect("the sidecar directory is readable"),
            "no history, no question"
        );
        assert!(!prompt.is_open());

        // An autosave with no project file beside it is offered whatever the
        // clock says, which is what makes this testable without waiting out
        // the recovery tolerance.
        let store = SnapshotStore::for_project(&project_file).expect("the path names a file");
        store
            .write(&Project::new("Unsaved"), 4, 10)
            .expect("the snapshot is written");
        assert!(
            prompt
                .open_for(&project_file)
                .expect("the sidecar directory is readable"),
            "an autosave ahead of the file raises the prompt"
        );
        assert!(prompt.is_open());
        let label = prompt.label().expect("the prompt states the choice");
        assert!(
            label.contains("revision 4"),
            "it names the autosave: {label}"
        );
        let recovered = prompt
            .recover()
            .expect("an open prompt answers")
            .project()
            .expect("recovering hands the autosaved project back");
        assert_eq!(recovered.name, "Unsaved");
        assert!(!prompt.is_open(), "answering closes the prompt");
    }

    #[test]
    fn a_closed_prompt_answers_nothing() {
        let mut prompt = RecoveryPrompt::new();
        assert!(!prompt.is_open());
        assert!(prompt.label().is_none());
        assert!(prompt.recovery().is_none());
        assert!(prompt.recover().is_none());
        assert!(prompt.discard().is_none());
        assert!(prompt.error().is_none());
    }

    #[test]
    fn an_outcome_carries_either_a_project_or_a_reason() {
        let recovered = RecoveryOutcome::Recovered(Box::new(Project::new("Doc cut")));
        assert!(recovered.error().is_none());
        assert_eq!(
            recovered.project().expect("a project came back").name,
            "Doc cut"
        );
        assert!(RecoveryOutcome::Discarded.project().is_none());
        let failed = RecoveryOutcome::Failed(sub_core::SubError::new(
            sub_edit::codes::AUTOSAVE_FAILED,
            "no",
        ));
        assert_eq!(
            failed.error().expect("a reason came back").code,
            sub_edit::codes::AUTOSAVE_FAILED
        );
        assert!(failed.project().is_none());
    }
}
