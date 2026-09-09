//! Autosave and snapshot history against a running engine.
//!
//! These tests judge behaviour, not code shape: files appearing in the sidecar
//! directory, the engine staying responsive while they are written, the
//! history pruning to K, an unsaved session being offered back on the next
//! open, and an entry from the restore list loading as the project it holds.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use sub_edit::autosave::{
    Autosave, AutosaveConfig, Snapshot, SnapshotStore, check_for_recovery,
    check_for_recovery_within, sidecar_dir,
};
use sub_edit::commands::RenameSequence;
use sub_edit::{Engine, EngineHandle};
use sub_model::{Project, Sequence, SequenceId, SequenceSettings, json};

/// How long a test waits for the worker before it calls the behaviour missing.
const PATIENCE: Duration = Duration::from_secs(10);

/// The interval the tests autosave at: short enough to keep them quick, long
/// enough that "once per interval" is still observable.
const INTERVAL: Duration = Duration::from_millis(40);

/// A directory of this test's own, emptied first so a rerun starts clean.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-edit-autosave-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the temporary directory is creatable");
    dir
}

/// A project with one sequence, and the identifier of that sequence.
fn project() -> (Project, SequenceId) {
    let mut project = Project::new("Doc cut");
    let sequence = Sequence::new("Main", SequenceSettings::default());
    let id = sequence.id;
    project.sequences.push(sequence);
    (project, id)
}

/// Renames the sequence, which is one undoable command and so one revision.
fn rename(handle: &EngineHandle, sequence: SequenceId, name: &str) {
    handle
        .apply(RenameSequence {
            sequence,
            name: name.to_owned(),
        })
        .expect("the rename applies");
}

/// Waits for `condition`, failing the test with `what` when it never holds.
fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn a_change_is_autosaved_to_the_sidecar_directory_within_an_interval() {
    let dir = temp_dir("writes");
    let project_file = dir.join("doc-cut.sub");
    let (project, sequence) = project();
    std::fs::write(&project_file, json::to_json(&project).expect("json")).expect("save");

    let engine = Engine::spawn(project).expect("engine");
    let store = SnapshotStore::for_project(&project_file).expect("store");
    let autosave = Autosave::spawn(
        engine.handle(),
        store.clone(),
        AutosaveConfig {
            interval: INTERVAL,
            keep: 4,
        },
    )
    .expect("autosave");

    assert_eq!(
        store.dir(),
        sidecar_dir(&project_file)
            .expect("sidecar")
            .join("autosave"),
        "snapshots go into the project's sidecar directory"
    );

    rename(engine.handle(), sequence, "Assembly");
    wait_for("the first autosave", || {
        !store.list().expect("list").is_empty()
    });

    let snapshots = store.list().expect("list");
    assert_eq!(snapshots.len(), 1, "one change is one snapshot");
    let restored = snapshots[0].load().expect("load");
    assert_eq!(restored.sequences[0].name, "Assembly");
    assert_eq!(snapshots[0].revision(), 1);

    autosave.stop().expect("stop");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unchanged_project_is_never_autosaved() {
    let dir = temp_dir("unchanged");
    let (project, _) = project();
    let engine = Engine::spawn(project).expect("engine");
    let store = SnapshotStore::new(dir.join("autosave"));
    let autosave = Autosave::spawn(
        engine.handle(),
        store.clone(),
        AutosaveConfig {
            interval: INTERVAL,
            keep: 4,
        },
    )
    .expect("autosave");

    // Several intervals with nothing to save.
    std::thread::sleep(INTERVAL * 5);
    assert!(
        store.list().expect("list").is_empty(),
        "autosave writes only after a change"
    );
    assert_eq!(autosave.status().written, 0);
    assert!(!autosave.status().pending);

    autosave.stop().expect("stop");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn many_changes_inside_one_interval_collapse_into_one_snapshot() {
    let dir = temp_dir("collapse");
    let (project, sequence) = project();
    let engine = Engine::spawn(project).expect("engine");
    let store = SnapshotStore::new(dir.join("autosave"));
    let autosave = Autosave::spawn(
        engine.handle(),
        store.clone(),
        AutosaveConfig {
            interval: Duration::from_secs(3),
            keep: 8,
        },
    )
    .expect("autosave");

    for take in 0..20 {
        rename(engine.handle(), sequence, &format!("take {take}"));
    }
    wait_for("the worker to see every change", || {
        autosave.status().seen_revision == 20
    });
    assert_eq!(
        store.list().expect("list").len(),
        0,
        "nothing is written before the interval elapses"
    );

    // Stopping flushes the pending change as a single snapshot.
    let store = autosave.stop().expect("stop");
    let snapshots = store.list().expect("list");
    assert_eq!(snapshots.len(), 1, "20 changes, one snapshot");
    assert_eq!(snapshots[0].revision(), 20, "the snapshot holds the latest");
    assert_eq!(
        snapshots[0].load().expect("load").sequences[0].name,
        "take 19"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn autosaving_never_blocks_the_thread_that_edits() {
    let dir = temp_dir("non-blocking");
    let (project, sequence) = project();
    let engine = Engine::spawn(project).expect("engine");
    let store = SnapshotStore::new(dir.join("autosave"));
    let autosave = Autosave::spawn(
        engine.handle(),
        store.clone(),
        AutosaveConfig {
            // Every poll is due, so the worker is writing snapshots for the
            // whole of the loop below.
            interval: Duration::from_millis(1),
            keep: 3,
        },
    )
    .expect("autosave");

    // The editing thread stands in for the UI: it only submits commands, and
    // never calls anything in the autosave module.
    let (done, edits) = mpsc::channel();
    let handle = engine.handle().clone();
    let editor = std::thread::spawn(move || {
        let mut slowest = Duration::ZERO;
        for take in 0..200 {
            let started = Instant::now();
            rename(&handle, sequence, &format!("take {take}"));
            slowest = slowest.max(started.elapsed());
        }
        done.send(slowest).expect("the result is sent");
    });

    let slowest = edits
        .recv_timeout(PATIENCE)
        .expect("the editing thread is never blocked by autosave");
    editor.join().expect("the editing thread finishes");

    assert!(
        slowest < Duration::from_millis(500),
        "the slowest command took {slowest:?}, so autosave was in its way"
    );
    wait_for("at least one snapshot during the edits", || {
        autosave.status().written > 0
    });
    assert!(
        autosave.status().last_error.is_none(),
        "no write failed: {:?}",
        autosave.status().last_error
    );

    let store = autosave.stop().expect("stop");
    // How many snapshots land depends on how fast the machine runs the 200
    // edits: a quick one finishes them in fewer than K intervals. What the
    // test is about is the bound, so assert that and not an exact count.
    let kept = store.list().expect("list").len();
    assert!(
        (1..=3).contains(&kept),
        "the history is still bounded by K, got {kept}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_restore_list_keeps_the_last_k_autosaves_newest_first() {
    let dir = temp_dir("restore-list");
    let (project, sequence) = project();
    let engine = Engine::spawn(project).expect("engine");
    let store = SnapshotStore::new(dir.join("autosave"));
    let autosave = Autosave::spawn(
        engine.handle(),
        store.clone(),
        AutosaveConfig {
            interval: INTERVAL,
            keep: 3,
        },
    )
    .expect("autosave");

    for take in 1..=6 {
        rename(engine.handle(), sequence, &format!("take {take}"));
        let written = autosave.status().written;
        wait_for("the next autosave", || autosave.status().written > written);
    }
    let store = autosave.stop().expect("stop");

    let snapshots = store.list().expect("list");
    assert_eq!(snapshots.len(), 3, "only the last K are kept");
    let revisions: Vec<u64> = snapshots.iter().map(Snapshot::revision).collect();
    let mut newest_first = revisions.clone();
    newest_first.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(revisions, newest_first, "a menu reads them newest first");

    // Every entry a menu offers restores the project it was taken from.
    for snapshot in &snapshots {
        let restored = snapshot.load().expect("load");
        assert_eq!(
            restored.sequences[0].name,
            format!("take {}", snapshot.revision()),
            "{} restores what it holds",
            snapshot.label()
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_autosave_newer_than_the_project_file_offers_recovery() {
    let dir = temp_dir("recovery");
    let project_file = dir.join("doc-cut.sub");
    let (project, sequence) = project();
    std::fs::write(&project_file, json::to_json(&project).expect("json")).expect("save");

    assert!(
        check_for_recovery(&project_file).expect("check").is_none(),
        "a project with no snapshots opens silently"
    );

    // A session that edits and then dies without saving, a moment later.
    std::thread::sleep(Duration::from_millis(10));
    let engine = Engine::spawn(project).expect("engine");
    let store = SnapshotStore::for_project(&project_file).expect("store");
    let autosave = Autosave::spawn(
        engine.handle(),
        store,
        AutosaveConfig {
            interval: INTERVAL,
            keep: 4,
        },
    )
    .expect("autosave");
    rename(engine.handle(), sequence, "Assembly");
    let store = autosave.stop().expect("stop");
    engine.shutdown().expect("shutdown");
    assert_eq!(store.list().expect("list").len(), 1);

    // The snapshot is milliseconds, not seconds, newer than the file, so the
    // check runs with a tolerance small enough to see that.
    let tolerance = Duration::from_millis(1);
    let recovery = check_for_recovery_within(&project_file, tolerance)
        .expect("check")
        .expect("the newer autosave is offered");
    assert_eq!(recovery.project_file(), project_file);
    assert!(
        recovery.label().contains("newer than the file on disk"),
        "the prompt says what happened: {}",
        recovery.label()
    );
    assert!(recovery.ahead_by().is_some());

    // Recovering opens the autosaved project, not the stale file.
    let recovered = recovery.recover().expect("recover");
    assert_eq!(recovered.sequences[0].name, "Assembly");
    let on_disk = json::from_json(&std::fs::read_to_string(&project_file).expect("read"))
        .expect("the project file still parses");
    assert_eq!(on_disk.sequences[0].name, "Main", "the file is untouched");

    // Discarding drops the history so the next open does not ask again.
    assert_eq!(recovery.discard().expect("discard"), 1);
    assert!(
        check_for_recovery_within(&project_file, tolerance)
            .expect("check")
            .is_none()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_project_saved_after_its_last_autosave_opens_without_a_prompt() {
    let dir = temp_dir("saved-after");
    let project_file = dir.join("doc-cut.sub");
    let (project, _) = project();
    let store = SnapshotStore::for_project(&project_file).expect("store");
    store.write(&project, 4, 4).expect("write");

    // The user saved just after that autosave. A file's modification time and
    // the clock a snapshot is stamped with disagree by a little, so this is
    // exactly the case the default tolerance exists for.
    std::fs::write(&project_file, json::to_json(&project).expect("json")).expect("save");
    assert!(
        SystemTime::now()
            .duration_since(store.latest().expect("latest").expect("one").saved_at())
            .expect("the snapshot is in the past")
            < Duration::from_secs(1),
        "the save follows the autosave inside the tolerance"
    );

    assert!(
        check_for_recovery(&project_file).expect("check").is_none(),
        "a project saved after its last autosave opens silently"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn snapshots_without_a_project_file_are_offered_too() {
    let dir = temp_dir("no-project-file");
    let project_file = dir.join("doc-cut.sub");
    let (project, _) = project();
    SnapshotStore::for_project(&project_file)
        .expect("store")
        .write(&project, 2, 4)
        .expect("write");

    let recovery = check_for_recovery(&project_file)
        .expect("check")
        .expect("a snapshot with no project file is still recoverable");
    assert!(recovery.project_modified().is_none());
    assert!(recovery.label().contains("project file does not"));
    assert_eq!(recovery.recover().expect("recover").sequences.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
