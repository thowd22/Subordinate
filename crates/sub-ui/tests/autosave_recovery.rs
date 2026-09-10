//! The autosave recovery prompt and the snapshot restore list, driven through
//! the shared `egui_kittest` harness.
//!
//! Everything here runs over a real sidecar directory written by
//! `sub_edit::autosave`, so the snapshots the prompt offers and the list
//! restores are the same files the background worker writes. The interaction
//! tests click "Recover" and "Discard" by their accessibility labels and then
//! assert on the model — the project handed back, and what is left on disk —
//! rather than on pixels. One snapshot test holds the dialog to a committed
//! PNG; it is skipped, and reported, on a machine with no wgpu adapter.

mod support;

use std::path::{Path, PathBuf};

use egui_kittest::kittest::Queryable;
use sub_edit::autosave::{SnapshotStore, check_for_recovery, sidecar_dir};
use sub_model::Project;
use sub_ui::recovery::{
    DISCARD_LABEL, RECOVER_LABEL, RecoveryOutcome, RecoveryPrompt, SnapshotMenu, entry_label,
};

/// A directory of this test's own, emptied first so a rerun starts clean.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-ui-autosave-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the temporary directory is creatable");
    dir
}

/// Writes `project` to `path` as a real `.sub` file.
fn save(project: &Project, path: &Path) {
    let text = sub_model::json::to_json(project).expect("the project serialises");
    std::fs::write(path, text).expect("the project file is writable");
}

/// Moves `path`'s modification time `by` into the past.
fn backdate(path: &Path, by: std::time::Duration) {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("the project file is writable");
    let when = std::time::SystemTime::now() - by;
    file.set_modified(when)
        .expect("the modification time is settable");
}

/// The store holding `project_file`'s autosave history.
fn store_for(project_file: &Path) -> SnapshotStore {
    SnapshotStore::for_project(project_file).expect("the path names a file")
}

/// A project file with `revisions` autosaves ahead of it, newest last.
///
/// The saved file holds "Saved"; every snapshot is named for the revision it
/// was taken at, so a restored project says which snapshot it came from. The
/// project file's own modification time is pushed back by writing it first and
/// then autosaving, which is exactly the shape of a session that ended without
/// saving.
fn project_with_history(dir: &Path, revisions: u64, keep: usize) -> (PathBuf, SnapshotStore) {
    let project_file = dir.join("doc-cut.sub");
    save(&Project::new("Saved"), &project_file);
    // A snapshot is stamped from the wall clock floored to a millisecond,
    // while the file carries whatever sub-millisecond modification time the
    // filesystem recorded. On a fast machine the whole sequence below runs
    // inside one millisecond, so the snapshots can come out stamped *behind*
    // the file and nothing looks stale (seen on the macOS CI runner). Push the
    // file's own time back so "the session ended without saving" is
    // unambiguous, which is the shape these tests mean to set up.
    backdate(&project_file, std::time::Duration::from_secs(5));
    let store = store_for(&project_file);
    for revision in 1..=revisions {
        let project = Project::new(format!("Revision {revision}"));
        store
            .write(&project, revision, keep)
            .expect("the snapshot is written");
    }
    (project_file, store)
}

/// The state the interaction tests drive and then assert on.
struct Fixture {
    prompt: RecoveryPrompt,
    menu: SnapshotMenu,
    outcome: Option<RecoveryOutcome>,
}

impl Fixture {
    /// A prompt opened on `project_file`'s recovery, and a menu listing its
    /// whole history.
    fn open(project_file: &Path) -> Self {
        // The tolerance in `check_for_recovery` exists so a save made moments
        // after an autosave does not prompt; a test writes both inside a
        // millisecond, so it asks the question with no slack at all.
        Self::open_within(project_file, std::time::Duration::ZERO)
    }

    /// The same, allowing a save made this recently to count as newer than the
    /// autosaves before it — what the shipped tolerance is for.
    fn open_within(project_file: &Path, tolerance: std::time::Duration) -> Self {
        let mut prompt = RecoveryPrompt::new();
        let recovery = sub_edit::autosave::check_for_recovery_within(project_file, tolerance)
            .expect("the sidecar directory is readable");
        prompt.open(recovery);
        let mut menu = SnapshotMenu::new();
        menu.set_project(project_file).expect("the history lists");
        Self {
            prompt,
            menu,
            outcome: None,
        }
    }
}

/// Paints the prompt and the restore list into one frame.
fn body(ui: &mut eframe::egui::Ui, fixture: &mut Fixture) {
    if let Some(answer) = fixture.prompt.body_ui(ui) {
        fixture.outcome = Some(answer);
    }
    ui.separator();
    ui.label(sub_ui::recovery::MENU_TITLE);
    if let Some(restored) = fixture.menu.ui(ui) {
        fixture.outcome = Some(restored);
    }
}

#[test]
fn the_prompt_offers_the_newest_autosave_and_recovering_hands_it_back() {
    let dir = temp_dir("recover");
    let (project_file, store) = project_with_history(&dir, 3, 10);

    let fixture = Fixture::open(&project_file);
    assert!(fixture.prompt.is_open(), "an autosave is ahead of the file");
    let label = fixture
        .prompt
        .label()
        .expect("the prompt states the choice");
    assert!(
        label.contains("newer than the file on disk"),
        "the prompt says what happened: {label}"
    );
    assert!(label.contains("revision 3"), "and which autosave: {label}");

    let mut harness = support::panel_harness_state(fixture, body);
    harness.run();
    harness.get_by_label(RECOVER_LABEL).click();
    harness.run();

    let fixture = harness.state();
    let recovered = fixture
        .outcome
        .as_ref()
        .expect("the click answered the prompt");
    assert!(recovered.error().is_none(), "the newest snapshot loaded");
    assert!(!fixture.prompt.is_open(), "answering closes the prompt");

    let RecoveryOutcome::Recovered(project) = recovered else {
        panic!("recovering hands a project back, not {recovered:?}");
    };
    assert_eq!(
        project.name, "Revision 3",
        "the newest autosave is the one offered"
    );
    // Recovering destroys nothing: the file is untouched and so is the history.
    let on_disk = std::fs::read_to_string(&project_file).expect("the file is still there");
    assert_eq!(
        sub_model::json::from_json(&on_disk)
            .expect("it still parses")
            .name,
        "Saved"
    );
    assert_eq!(store.list().expect("the history lists").len(), 3);
}

#[test]
fn discarding_drops_the_history_and_the_next_open_asks_nothing() {
    let dir = temp_dir("discard");
    let (project_file, store) = project_with_history(&dir, 2, 10);

    let mut harness = support::panel_harness_state(Fixture::open(&project_file), body);
    harness.run();
    harness.get_by_label(DISCARD_LABEL).click();
    harness.run();

    let fixture = harness.state();
    let outcome = fixture.outcome.as_ref().expect("the click answered");
    assert!(
        matches!(outcome, RecoveryOutcome::Discarded),
        "discarding recovers nothing, it got {outcome:?}"
    );
    assert!(!fixture.prompt.is_open(), "answering closes the prompt");

    assert!(
        store.list().expect("the directory still lists").is_empty(),
        "the whole history went"
    );
    assert!(
        check_for_recovery(&project_file)
            .expect("the sidecar directory is readable")
            .is_none(),
        "so the next open asks nothing"
    );
    let on_disk = std::fs::read_to_string(&project_file).expect("the project file was kept");
    assert_eq!(
        sub_model::json::from_json(&on_disk)
            .expect("it still parses")
            .name,
        "Saved"
    );
}

#[test]
fn the_restore_list_shows_the_kept_autosaves_newest_first_and_restores_the_one_clicked() {
    let dir = temp_dir("restore");
    // Six autosaves, a history three deep: the three oldest are pruned.
    let (project_file, _) = project_with_history(&dir, 6, 3);

    let fixture = Fixture::open(&project_file);
    let labels = fixture.menu.entry_labels();
    assert_eq!(labels.len(), 3, "the list keeps K entries: {labels:?}");
    let revisions: Vec<u64> = fixture
        .menu
        .snapshots()
        .iter()
        .map(sub_edit::autosave::Snapshot::revision)
        .collect();
    assert_eq!(revisions, vec![6, 5, 4], "newest first");
    // The middle entry, so the click is not the newest one the prompt offers.
    let wanted = entry_label(&fixture.menu.snapshots()[1], std::time::SystemTime::now());

    let mut harness = support::panel_harness_state(fixture, body);
    harness.run();
    harness.get_by_label(wanted.as_str()).click();
    harness.run();

    let outcome = harness
        .state()
        .outcome
        .as_ref()
        .expect("the click restored something");
    let RecoveryOutcome::Recovered(project) = outcome else {
        panic!("a restore hands a project back, not {outcome:?}");
    };
    assert_eq!(
        project.name, "Revision 5",
        "the entry clicked is the project restored"
    );
}

#[test]
fn a_project_saved_after_its_last_autosave_shows_no_prompt_but_still_lists_its_history() {
    let dir = temp_dir("saved");
    let (project_file, _) = project_with_history(&dir, 2, 10);
    // Saving again, after the autosaves, is the ordinary end of a session.
    save(&Project::new("Saved later"), &project_file);

    // At the shipped tolerance, which is what an open really uses: the file's
    // recorded modification time can read a millisecond or two behind the
    // clock the snapshot before it was stamped with.
    let fixture = Fixture::open_within(&project_file, sub_edit::autosave::RECOVERY_TOLERANCE);
    assert!(!fixture.prompt.is_open(), "nothing was lost, nothing asked");
    assert_eq!(
        fixture.menu.snapshots().len(),
        2,
        "the history is still there to restore from"
    );

    let mut harness = support::panel_harness_state(fixture, body);
    harness.run();
    assert!(
        harness.query_by_label(RECOVER_LABEL).is_none(),
        "a closed prompt paints no buttons"
    );
    assert!(harness.query_by_label(DISCARD_LABEL).is_none());
    assert!(harness.state().outcome.is_none());
}

#[test]
fn a_menu_with_no_autosaves_says_so() {
    let dir = temp_dir("empty");
    let project_file = dir.join("doc-cut.sub");
    save(&Project::new("Saved"), &project_file);

    let mut menu = SnapshotMenu::new();
    menu.set_project(&project_file).expect("nothing to list");
    assert!(menu.snapshots().is_empty());
    assert_eq!(
        menu.project_file(),
        Some(project_file.as_path()),
        "the menu knows whose history it shows"
    );
    // The store's directory is the documented sidecar location.
    let expected = sidecar_dir(&project_file)
        .expect("the path names a file")
        .join(sub_edit::autosave::AUTOSAVE_DIR_NAME);
    assert_eq!(store_for(&project_file).dir(), expected);

    let mut harness = support::panel_harness_state((menu, None::<RecoveryOutcome>), |ui, state| {
        state.1 = state.0.ui(ui);
    });
    harness.run();
    assert!(
        harness
            .query_by_label(sub_ui::recovery::EMPTY_LABEL)
            .is_some(),
        "the list says it is empty"
    );
    assert!(harness.state().1.is_none());
}

#[test]
fn the_dialog_matches_its_snapshot() {
    // A snapshot needs an adapter; an interaction test above does not.
    if !support::can_render() {
        return;
    }
    let dir = temp_dir("snapshot");
    // No project file at all: the prompt's other wording, and the only shape
    // whose text is fully determined — with a file on disk, "you would lose N
    // seconds of work" depends on how long the test took to get here.
    let project_file = dir.join("doc-cut.sub");
    let store = store_for(&project_file);
    for revision in 1..=3 {
        store
            .write(&Project::new(format!("Revision {revision}")), revision, 10)
            .expect("the snapshot is written");
    }

    let mut harness = support::panel_harness_state(Fixture::open(&project_file), body);
    harness.run();
    support::snapshot(&mut harness, "autosave_recovery_prompt");
}
