//! Autosave and snapshot history: crash recovery and cheap versioning.
//!
//! An edit session that dies — a crash, a power cut, a killed process — must
//! not cost the work since the last explicit save, so the project is written
//! to its sidecar directory in the background (docs/PLAN.md §5.6). Three
//! pieces do that, and none of them touches the UI thread:
//!
//! - [`SnapshotStore`] owns `name.sub.d/autosave/` beside the project file and
//!   writes one deterministic `.sub` file per snapshot, to a `.tmp` that is
//!   renamed into place so a reader never sees half a file. It keeps the last
//!   K snapshots and drops the rest.
//! - [`Autosave`] is the worker: it subscribes to the engine's change events,
//!   remembers the newest revision it has seen, and every `interval` writes a
//!   snapshot of the project — but only when something actually changed. The
//!   project it writes is an [`EngineHandle::snapshot`] `Arc`, so the engine
//!   thread is never blocked and the caller never waits on a file system.
//! - [`check_for_recovery`] is what opening a project asks first: a snapshot
//!   newer than the project file on disk means the last session did not save,
//!   and the returned [`Recovery`] carries everything a prompt needs plus the
//!   two answers, [`Recovery::recover`] and [`Recovery::discard`].
//!
//! Restoring is deliberately not a [`crate::Command`]: a snapshot is a whole
//! project, not a mutation of the open one, so [`Snapshot::load`] hands back a
//! `Project` for the caller to open — with its own [`crate::Engine`] and a
//! fresh history — exactly as opening a file does.
//!
//! ```
//! use std::time::Duration;
//! use sub_edit::Engine;
//! use sub_edit::autosave::{Autosave, AutosaveConfig, SnapshotStore};
//! use sub_model::Project;
//!
//! let dir = std::env::temp_dir().join("sub-autosave-doc-example");
//! let _ = std::fs::remove_dir_all(&dir);
//! let store = SnapshotStore::new(&dir);
//!
//! // Nothing has been autosaved yet, so there is nothing to recover.
//! assert!(store.latest().unwrap().is_none());
//!
//! let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
//! let config = AutosaveConfig {
//!     interval: Duration::from_millis(10),
//!     keep: 5,
//! };
//! let autosave = Autosave::spawn(engine.handle(), store, config).unwrap();
//! // ... editing happens here, on another thread ...
//! let store = autosave.stop().unwrap();
//! assert!(store.list().unwrap().is_empty(), "an unchanged project is not autosaved");
//! # let _ = std::fs::remove_dir_all(&dir);
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sub_core::{SubError, SubResult};
use sub_model::{Project, json};

use crate::codes;
use crate::engine::EngineHandle;
use crate::event::ChangeEvent;

/// The sidecar subdirectory autosave snapshots live in.
pub const AUTOSAVE_DIR_NAME: &str = "autosave";

/// The extension a project file, and so a snapshot of one, carries.
pub const PROJECT_EXTENSION: &str = "sub";

/// How many snapshots [`AutosaveConfig::default`] keeps.
pub const DEFAULT_KEEP: usize = 10;

/// How often [`AutosaveConfig::default`] writes one, when something changed.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(30);

/// The longest the worker waits between checks, so a stop is prompt even when
/// the interval is minutes.
const MAX_POLL: Duration = Duration::from_millis(25);

/// How much newer than the project file a snapshot must be before
/// [`check_for_recovery`] offers it.
///
/// A file's modification time and the clock a snapshot is stamped with do not
/// agree to the microsecond — they come from different sources, and a file
/// system may keep whole seconds only — so a save made moments after an
/// autosave can look older than it is. Recovery is offered only when the
/// snapshot is ahead by more than this, because a needless prompt asking a user
/// to choose between two identical projects is worse than not asking.
pub const RECOVERY_TOLERANCE: Duration = Duration::from_secs(2);

/// One autosaved project on disk.
///
/// A snapshot is a complete `.sub` file: [`Snapshot::load`] reads it with the
/// same loader, and the same migrations, as any other project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    path: PathBuf,
    saved_at: SystemTime,
    revision: u64,
}

impl Snapshot {
    /// Where the snapshot is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// When it was written, as its file name records it.
    ///
    /// The time comes from the name rather than the file's metadata so a copied
    /// or restored sidecar directory keeps the history it describes.
    #[must_use]
    pub fn saved_at(&self) -> SystemTime {
        self.saved_at
    }

    /// The engine revision the project was at.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// A short description for a restore menu entry.
    ///
    /// The time is left to the caller to format: this crate has no locale and
    /// no calendar, and the UI has both.
    #[must_use]
    pub fn label(&self) -> String {
        format!("Autosave at revision {}", self.revision)
    }

    /// Reads the snapshot back as a project.
    ///
    /// # Errors
    ///
    /// - `edit.snapshot_not_found` when the file has gone since it was listed.
    /// - `edit.autosave_failed` when it cannot be read.
    /// - Whatever `sub_model::json::from_json` returns for its contents.
    pub fn load(&self) -> SubResult<Project> {
        let text = std::fs::read_to_string(&self.path).map_err(|err| {
            let code = if err.kind() == std::io::ErrorKind::NotFound {
                codes::SNAPSHOT_NOT_FOUND
            } else {
                codes::AUTOSAVE_FAILED
            };
            SubError::wrap(code, "an autosave snapshot cannot be read", &err)
                .with_detail("path", self.path.display().to_string())
        })?;
        json::from_json(&text)
    }

    /// Deletes the snapshot file.
    ///
    /// # Errors
    ///
    /// Returns `edit.autosave_failed` when the file exists but cannot be
    /// removed; a snapshot that is already gone is not an error.
    pub fn delete(&self) -> SubResult<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => {
                Err(
                    SubError::wrap(codes::AUTOSAVE_FAILED, "a snapshot cannot be deleted", &err)
                        .with_detail("path", self.path.display().to_string()),
                )
            }
        }
    }

    /// Parses `autosave-<millis>-r<revision>.sub`, returning `None` for any
    /// other name so foreign files in the directory are ignored rather than
    /// mistaken for history.
    fn from_file_name(dir: &Path, name: &str) -> Option<Self> {
        let rest = name.strip_prefix("autosave-")?;
        let rest = rest.strip_suffix(".sub")?;
        let (millis, revision) = rest.split_once("-r")?;
        let millis: u64 = millis.parse().ok()?;
        let revision: u64 = revision.parse().ok()?;
        Some(Self {
            path: dir.join(name),
            saved_at: UNIX_EPOCH + Duration::from_millis(millis),
            revision,
        })
    }

    /// The file name a snapshot taken at `millis` for `revision` gets.
    fn file_name(millis: u64, revision: u64) -> String {
        format!("autosave-{millis:013}-r{revision}.{PROJECT_EXTENSION}")
    }

    /// The key snapshots are ordered by: time first, then revision, so two
    /// snapshots written inside one millisecond still order by what they hold.
    fn order_key(&self) -> (SystemTime, u64) {
        (self.saved_at, self.revision)
    }
}

/// The autosave snapshots of one project, in its sidecar directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    /// A store over `dir`, which is created on the first write.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The store belonging to the project file at `project_file`:
    /// `doc-cut.sub` owns `doc-cut.sub.d/autosave/`.
    ///
    /// # Errors
    ///
    /// Returns `core.invalid_argument` when the path names no file.
    pub fn for_project(project_file: &Path) -> SubResult<Self> {
        Ok(Self::new(
            sidecar_dir(project_file)?.join(AUTOSAVE_DIR_NAME),
        ))
    }

    /// The directory the snapshots live in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every snapshot in the store, newest first: the order a restore menu
    /// lists them in.
    ///
    /// A store whose directory does not exist yet is empty, not an error.
    ///
    /// # Errors
    ///
    /// Returns `edit.autosave_failed` when the directory exists but cannot be
    /// read.
    pub fn list(&self) -> SubResult<Vec<Snapshot>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => {
                return Err(SubError::wrap(
                    codes::AUTOSAVE_FAILED,
                    "the autosave directory cannot be read",
                    &err,
                )
                .with_detail("path", self.dir.display().to_string()));
            }
        };
        let mut snapshots: Vec<Snapshot> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                Snapshot::from_file_name(&self.dir, name.to_str()?)
            })
            .collect();
        snapshots.sort_by_key(Snapshot::order_key);
        snapshots.reverse();
        Ok(snapshots)
    }

    /// The newest snapshot, if there is one.
    ///
    /// # Errors
    ///
    /// The same as [`SnapshotStore::list`].
    pub fn latest(&self) -> SubResult<Option<Snapshot>> {
        Ok(self.list()?.into_iter().next())
    }

    /// Writes one snapshot of `project` at `revision` and prunes the history to
    /// the newest `keep` snapshots.
    ///
    /// The bytes go to a `.tmp` beside the snapshot and are renamed into place,
    /// so a recovering session never reads a half-written project.
    ///
    /// # Errors
    ///
    /// - `core.invalid_argument` when `keep` is zero.
    /// - `edit.autosave_failed` when the directory or the file cannot be
    ///   written.
    /// - `core.internal` when the project cannot be serialised.
    pub fn write(&self, project: &Project, revision: u64, keep: usize) -> SubResult<Snapshot> {
        check_keep(keep)?;
        let text = json::to_json(project)?;
        std::fs::create_dir_all(&self.dir).map_err(|err| {
            SubError::wrap(
                codes::AUTOSAVE_FAILED,
                "the autosave directory cannot be created",
                &err,
            )
            .with_detail("path", self.dir.display().to_string())
        })?;

        let mut millis = unix_millis(SystemTime::now());
        let mut path = self.dir.join(Snapshot::file_name(millis, revision));
        // Two snapshots inside one millisecond would otherwise collide and the
        // older of the pair would be lost; step forward until the name is free.
        while path.exists() {
            millis = millis.saturating_add(1);
            path = self.dir.join(Snapshot::file_name(millis, revision));
        }

        let temporary = path.with_extension("tmp");
        let io = |err: &std::io::Error| {
            SubError::wrap(
                codes::AUTOSAVE_FAILED,
                "an autosave snapshot cannot be written",
                err,
            )
            .with_detail("path", path.display().to_string())
        };
        std::fs::write(&temporary, text).map_err(|err| io(&err))?;
        std::fs::rename(&temporary, &path).map_err(|err| {
            let _ = std::fs::remove_file(&temporary);
            io(&err)
        })?;

        let snapshot = Snapshot {
            path,
            saved_at: UNIX_EPOCH + Duration::from_millis(millis),
            revision,
        };
        self.prune(keep)?;
        Ok(snapshot)
    }

    /// Deletes all but the newest `keep` snapshots, returning how many went.
    ///
    /// # Errors
    ///
    /// - `core.invalid_argument` when `keep` is zero.
    /// - `edit.autosave_failed` when a snapshot cannot be listed or removed.
    pub fn prune(&self, keep: usize) -> SubResult<usize> {
        check_keep(keep)?;
        let snapshots = self.list()?;
        let mut removed = 0;
        for snapshot in snapshots.into_iter().skip(keep) {
            snapshot.delete()?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Deletes every snapshot, and the directory itself when it is left empty.
    ///
    /// This is what discarding a recovery does: the session is finished with,
    /// and its snapshots must not offer themselves again on the next open.
    ///
    /// # Errors
    ///
    /// Returns `edit.autosave_failed` when a snapshot cannot be listed or
    /// removed.
    pub fn clear(&self) -> SubResult<usize> {
        let snapshots = self.list()?;
        let removed = snapshots.len();
        for snapshot in snapshots {
            snapshot.delete()?;
        }
        let _ = std::fs::remove_dir(&self.dir);
        Ok(removed)
    }
}

/// The sidecar directory of a project file: `doc-cut.sub` owns
/// `doc-cut.sub.d/` beside it (docs/PLAN.md §5.6).
///
/// # Errors
///
/// Returns `core.invalid_argument` when the path names no file.
pub fn sidecar_dir(project_file: &Path) -> SubResult<PathBuf> {
    let name = project_file
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "a project path must name a file to have a sidecar directory",
            )
            .with_detail("path", project_file.display().to_string())
        })?;
    Ok(project_file.with_file_name(format!("{name}.d")))
}

/// What an open found: a snapshot newer than the project file itself.
///
/// The last session wrote this snapshot and then did not save, so the file on
/// disk is behind what the user had. A prompt shows [`Recovery::label`] and
/// offers the two answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    project_file: PathBuf,
    snapshot: Snapshot,
    project_modified: Option<SystemTime>,
    store: SnapshotStore,
}

impl Recovery {
    /// The project file the recovery is about.
    #[must_use]
    pub fn project_file(&self) -> &Path {
        &self.project_file
    }

    /// The snapshot that would be recovered: the newest one.
    #[must_use]
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// The store holding the whole snapshot history, so a prompt can also
    /// offer the older ones.
    #[must_use]
    pub fn store(&self) -> &SnapshotStore {
        &self.store
    }

    /// When the project file itself was last written, when that is known.
    #[must_use]
    pub fn project_modified(&self) -> Option<SystemTime> {
        self.project_modified
    }

    /// How far ahead of the saved project the snapshot is, when both times are
    /// known: the "you would lose this much work" a prompt states.
    #[must_use]
    pub fn ahead_by(&self) -> Option<Duration> {
        self.snapshot
            .saved_at
            .duration_since(self.project_modified?)
            .ok()
    }

    /// A one-line description of the choice, for the prompt's title.
    #[must_use]
    pub fn label(&self) -> String {
        if self.project_modified.is_some() {
            format!(
                "An autosave of this project is newer than the file on disk ({}).",
                self.snapshot.label()
            )
        } else {
            format!(
                "An autosave of this project exists but the project file does not ({}).",
                self.snapshot.label()
            )
        }
    }

    /// Answers "recover": the snapshot, loaded as the project to open.
    ///
    /// The snapshot is left in place — nothing is destroyed until the user
    /// saves over the project file — and the caller opens the returned project
    /// with a fresh engine and history.
    ///
    /// # Errors
    ///
    /// The same as [`Snapshot::load`].
    pub fn recover(&self) -> SubResult<Project> {
        self.snapshot.load()
    }

    /// Answers "discard": drops the whole snapshot history so the next open
    /// does not ask again, and leaves the project file untouched.
    ///
    /// # Errors
    ///
    /// Returns `edit.autosave_failed` when a snapshot cannot be removed.
    pub fn discard(&self) -> SubResult<usize> {
        self.store.clear()
    }
}

/// Asks whether opening `project_file` should offer to recover.
///
/// Returns `Some` when the newest autosave snapshot is newer than the project
/// file — the signature of a session that ended without saving — or when
/// snapshots exist for a project file that is gone. A project saved after its
/// last autosave, and a project with no snapshots at all, return `None` and
/// open silently.
///
/// The snapshot must be ahead by more than [`RECOVERY_TOLERANCE`]; use
/// [`check_for_recovery_within`] to say how much slack to allow.
///
/// # Errors
///
/// - `core.invalid_argument` when the path names no file.
/// - `edit.autosave_failed` when the sidecar directory cannot be read.
pub fn check_for_recovery(project_file: &Path) -> SubResult<Option<Recovery>> {
    check_for_recovery_within(project_file, RECOVERY_TOLERANCE)
}

/// [`check_for_recovery`] with an explicit clock and file system tolerance.
///
/// # Errors
///
/// The same as [`check_for_recovery`].
pub fn check_for_recovery_within(
    project_file: &Path,
    tolerance: Duration,
) -> SubResult<Option<Recovery>> {
    let store = SnapshotStore::for_project(project_file)?;
    let Some(snapshot) = store.latest()? else {
        return Ok(None);
    };
    let project_modified = std::fs::metadata(project_file)
        .and_then(|metadata| metadata.modified())
        .ok();
    let stale = match project_modified {
        Some(modified) => snapshot.saved_at > modified + tolerance,
        None => true,
    };
    if !stale {
        return Ok(None);
    }
    Ok(Some(Recovery {
        project_file: project_file.to_path_buf(),
        snapshot,
        project_modified,
        store,
    }))
}

/// How the autosave worker behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutosaveConfig {
    /// How long after a change a snapshot is written. Changes inside one
    /// interval collapse into a single snapshot.
    pub interval: Duration,
    /// How many snapshots the history keeps.
    pub keep: usize,
}

impl Default for AutosaveConfig {
    fn default() -> Self {
        Self {
            interval: DEFAULT_INTERVAL,
            keep: DEFAULT_KEEP,
        }
    }
}

/// What the autosave worker has done so far, as a status line reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutosaveStatus {
    /// How many snapshots have been written this session.
    pub written: u64,
    /// The revision of the newest snapshot, or zero before the first one.
    pub saved_revision: u64,
    /// The newest revision seen from the engine.
    pub seen_revision: u64,
    /// Whether a change is waiting for the next interval.
    pub pending: bool,
    /// The last write failure, if any: a full or read-only sidecar is reported
    /// here, never raised, because it must not stop an edit.
    pub last_error: Option<String>,
}

/// The state the worker thread and its handle share.
#[derive(Debug)]
struct Shared {
    stop: AtomicBool,
    written: AtomicU64,
    saved_revision: AtomicU64,
    seen_revision: AtomicU64,
    last_error: Mutex<Option<String>>,
}

/// The background autosave worker.
///
/// Dropping it stops the thread and joins it, after a final snapshot of any
/// change the last interval did not reach.
#[derive(Debug)]
pub struct Autosave {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    store: SnapshotStore,
}

impl Autosave {
    /// Starts autosaving the project `handle` owns into `store`.
    ///
    /// The worker subscribes to the engine's change events before it returns,
    /// so no change made after this call is missed. Every snapshot is taken
    /// from an [`EngineHandle::snapshot`] `Arc`: the write happens on the
    /// worker thread, off both the UI thread and the engine thread.
    ///
    /// # Errors
    ///
    /// - `core.invalid_argument` when the interval is zero or `keep` is zero.
    /// - `core.internal` when the worker thread cannot be started.
    pub fn spawn(
        handle: &EngineHandle,
        store: SnapshotStore,
        config: AutosaveConfig,
    ) -> SubResult<Self> {
        check_keep(config.keep)?;
        if config.interval.is_zero() {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "the autosave interval must be greater than zero",
            ));
        }
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            written: AtomicU64::new(0),
            saved_revision: AtomicU64::new(0),
            seen_revision: AtomicU64::new(0),
            last_error: Mutex::new(None),
        });
        let events = handle.subscribe();
        let handle = handle.clone();
        let worker = Worker {
            shared: Arc::clone(&shared),
            store: store.clone(),
            config,
        };
        let thread = thread::Builder::new()
            .name("sub-autosave".to_owned())
            .spawn(move || worker.run(&handle, &events))
            .map_err(|err| {
                SubError::wrap(
                    sub_core::codes::INTERNAL,
                    "could not start the autosave thread",
                    &err,
                )
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
            store,
        })
    }

    /// The store the snapshots go to.
    #[must_use]
    pub fn store(&self) -> &SnapshotStore {
        &self.store
    }

    /// What the worker has done so far. Reading it never blocks the worker.
    #[must_use]
    pub fn status(&self) -> AutosaveStatus {
        let seen = self.shared.seen_revision.load(Ordering::Acquire);
        let saved = self.shared.saved_revision.load(Ordering::Acquire);
        AutosaveStatus {
            written: self.shared.written.load(Ordering::Acquire),
            saved_revision: saved,
            seen_revision: seen,
            pending: seen > saved,
            last_error: self
                .shared
                .last_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        }
    }

    /// Stops the worker and waits for it, writing a final snapshot first when a
    /// change has not been saved yet, and hands the store back.
    ///
    /// # Errors
    ///
    /// Returns `core.internal` when the worker thread panicked.
    pub fn stop(mut self) -> SubResult<SnapshotStore> {
        self.halt()?;
        Ok(self.store.clone())
    }

    /// The body of both [`Autosave::stop`] and the `Drop` implementation.
    fn halt(&mut self) -> SubResult<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        self.shared.stop.store(true, Ordering::Release);
        thread.join().map_err(|_| {
            SubError::new(
                sub_core::codes::INTERNAL,
                "the autosave thread panicked; snapshots have stopped",
            )
        })
    }
}

impl Drop for Autosave {
    fn drop(&mut self) {
        let _ = self.halt();
    }
}

/// The worker thread's own state.
struct Worker {
    shared: Arc<Shared>,
    store: SnapshotStore,
    config: AutosaveConfig,
}

impl Worker {
    /// Drains change events, and writes a snapshot at most once per interval
    /// and only when something changed.
    fn run(self, handle: &EngineHandle, events: &crate::EventReceiver) {
        let poll = self.config.interval.min(MAX_POLL);
        let mut due = SystemTime::now() + self.config.interval;
        while !self.shared.stop.load(Ordering::Acquire) {
            if let Some(event) = events.recv_timeout(poll) {
                self.see(&event);
                // Drain the rest of this burst without waiting again.
                while let Some(event) = events.try_recv() {
                    self.see(&event);
                }
            } else if events.is_closed() {
                break;
            }
            if SystemTime::now() >= due {
                self.snapshot(handle);
                due = SystemTime::now() + self.config.interval;
            }
        }
        // A final snapshot so the work of the last, incomplete interval is not
        // the one thing autosave loses.
        self.snapshot(handle);
    }

    /// Records that the engine reached the revision this event carries.
    fn see(&self, event: &ChangeEvent) {
        self.shared
            .seen_revision
            .fetch_max(event.revision, Ordering::AcqRel);
    }

    /// Writes one snapshot when the engine is ahead of the newest one.
    ///
    /// The engine's published revision, not the last event seen, decides:
    /// events only wake the worker, and one may still be in flight when a stop
    /// asks for the final snapshot.
    fn snapshot(&self, handle: &EngineHandle) {
        // The revision is read before the project, because the engine
        // publishes the project first: the snapshot taken is then at least as
        // new as the revision recorded for it, never older.
        let revision = handle.revision();
        self.shared
            .seen_revision
            .fetch_max(revision, Ordering::AcqRel);
        if revision == 0 || revision <= self.shared.saved_revision.load(Ordering::Acquire) {
            return;
        }
        // The project comes off the published snapshot slot, not the queue, so
        // neither the engine nor the UI waits for this write.
        let project = handle.snapshot();
        match self.store.write(&project, revision, self.config.keep) {
            Ok(_) => {
                self.shared
                    .saved_revision
                    .fetch_max(revision, Ordering::AcqRel);
                self.shared.written.fetch_add(1, Ordering::AcqRel);
                self.report(None);
            }
            Err(err) => {
                // A read-only or full sidecar must not stop an edit: the
                // failure is recorded and the next interval tries again.
                self.report(Some(err.to_string()));
            }
        }
    }

    /// Publishes the outcome of the last write attempt.
    fn report(&self, error: Option<String>) {
        *self
            .shared
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = error;
    }
}

/// Rejects a history that keeps nothing, which would delete each snapshot as
/// soon as it was written.
fn check_keep(keep: usize) -> SubResult<()> {
    if keep == 0 {
        return Err(SubError::new(
            sub_core::codes::INVALID_ARGUMENT,
            "an autosave history must keep at least one snapshot",
        ));
    }
    Ok(())
}

/// Milliseconds since the epoch, saturating for a clock set before it.
fn unix_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own, removed first so a rerun starts clean.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sub-autosave-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_project_file_owns_the_sidecar_directory_beside_it() {
        let sidecar = sidecar_dir(Path::new("/films/doc-cut.sub")).expect("sidecar");
        assert_eq!(sidecar, Path::new("/films/doc-cut.sub.d"));
        let store = SnapshotStore::for_project(Path::new("/films/doc-cut.sub")).expect("store");
        assert_eq!(store.dir(), Path::new("/films/doc-cut.sub.d/autosave"));
    }

    #[test]
    fn a_path_that_names_no_file_has_no_sidecar() {
        let err = sidecar_dir(Path::new("/")).expect_err("no file name");
        assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
    }

    #[test]
    fn a_snapshot_round_trips_through_the_store() {
        let dir = temp_dir("round-trip");
        let store = SnapshotStore::new(&dir);
        let project = Project::new("Doc cut");

        let snapshot = store.write(&project, 7, DEFAULT_KEEP).expect("write");
        assert_eq!(snapshot.revision(), 7);
        assert_eq!(snapshot.load().expect("load"), project);
        assert_eq!(store.list().expect("list"), vec![snapshot]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_history_keeps_the_newest_k_snapshots() {
        let dir = temp_dir("keep-k");
        let store = SnapshotStore::new(&dir);
        for revision in 1..=5 {
            let mut project = Project::new(format!("take {revision}"));
            project.name = format!("take {revision}");
            store.write(&project, revision, 3).expect("write");
        }

        let snapshots = store.list().expect("list");
        assert_eq!(snapshots.len(), 3, "only K snapshots are kept");
        let revisions: Vec<u64> = snapshots.iter().map(Snapshot::revision).collect();
        assert_eq!(revisions, vec![5, 4, 3], "the newest K, newest first");
        assert_eq!(
            snapshots[0].load().expect("load").name,
            "take 5",
            "each entry restores the project it was written from"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_history_that_keeps_nothing_is_refused() {
        let dir = temp_dir("keep-zero");
        let store = SnapshotStore::new(&dir);
        let err = store
            .write(&Project::new("Doc cut"), 1, 0)
            .expect_err("keep zero");
        assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
        assert_eq!(store.prune(0).expect_err("keep zero").code, err.code);
    }

    #[test]
    fn foreign_files_in_the_directory_are_not_history() {
        let dir = temp_dir("foreign");
        let store = SnapshotStore::new(&dir);
        store.write(&Project::new("Doc cut"), 1, 4).expect("write");
        std::fs::write(dir.join("notes.txt"), "hello").expect("write foreign file");
        std::fs::write(dir.join("autosave-nonsense.sub"), "{}").expect("write foreign file");

        assert_eq!(store.list().expect("list").len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_directory_lists_as_an_empty_history() {
        let store = SnapshotStore::new(temp_dir("missing"));
        assert!(store.list().expect("list").is_empty());
        assert!(store.latest().expect("latest").is_none());
    }

    #[test]
    fn a_deleted_snapshot_reports_snapshot_not_found() {
        let dir = temp_dir("deleted");
        let store = SnapshotStore::new(&dir);
        let snapshot = store.write(&Project::new("Doc cut"), 1, 4).expect("write");
        snapshot.delete().expect("delete");
        snapshot.delete().expect("deleting twice is not an error");

        let err = snapshot.load().expect_err("gone");
        assert_eq!(err.code, codes::SNAPSHOT_NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_removes_the_history_and_its_directory() {
        let dir = temp_dir("clear");
        let store = SnapshotStore::new(&dir);
        store.write(&Project::new("Doc cut"), 1, 4).expect("write");
        store.write(&Project::new("Doc cut"), 2, 4).expect("write");

        assert_eq!(store.clear().expect("clear"), 2);
        assert!(!dir.exists());
    }

    #[test]
    fn an_autosave_interval_of_zero_is_refused() {
        let engine = crate::Engine::spawn(Project::new("Doc cut")).expect("engine");
        let err = Autosave::spawn(
            engine.handle(),
            SnapshotStore::new(temp_dir("zero-interval")),
            AutosaveConfig {
                interval: Duration::ZERO,
                keep: 2,
            },
        )
        .expect_err("zero interval");
        assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
    }
}
