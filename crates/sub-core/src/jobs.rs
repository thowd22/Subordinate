//! The background job service: thumbnails, waveforms, proxies, PTS indexes and
//! plugin analyzers all run here rather than on the UI or engine thread
//! (docs/PLAN.md §5.2).
//!
//! A [`JobService`] owns a fixed pool of worker threads and a priority queue.
//! Work is submitted as a closure with a [`Priority`]; higher priorities run
//! first and jobs of equal priority run in submission order. Every job gets a
//! [`JobContext`] through which it reports progress and asks whether it has
//! been cancelled, and every state change is published as a [`JobEvent`] to
//! whoever subscribed — the UI draws progress from those events, and the MCP
//! bridge reports them to agents.
//!
//! Nothing here blocks the caller: [`JobService::submit`] returns a
//! [`JobHandle`] immediately. A handle can be cancelled at any point; a job
//! that has not started yet never runs, and a running job is asked to stop
//! through its [`CancelToken`], which is cooperative — long jobs must check
//! [`JobContext::is_cancelled`] between units of work.
//!
//! ```
//! use sub_core::jobs::{JobEvent, JobService, Priority};
//!
//! let service = JobService::new(2);
//! let events = service.subscribe();
//!
//! let handle = service.submit("demo", Priority::Normal, |ctx| {
//!     for step in 0..4 {
//!         ctx.progress(step + 1, 4);
//!     }
//!     Ok(())
//! });
//! assert!(handle.wait().is_completed());
//!
//! let progress: Vec<_> = events
//!     .try_iter()
//!     .filter_map(|event| match event {
//!         JobEvent::Progress { done, total, .. } => Some((done, total)),
//!         _ => None,
//!     })
//!     .collect();
//! assert_eq!(progress.last(), Some(&(4, 4)));
//! ```

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

use crate::error::{SubError, SubResult, codes};

/// A cooperative cancel flag shared with running work.
///
/// Cloning shares the flag, so a caller can hold one while a worker thread
/// holds another. Setting it is one store; work checks it between units and
/// returns [`codes::CANCELLED`] when it is set.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that has not been cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the work this token was handed to to stop.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether cancellation has been asked for.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// The error cancelled work reports, naming what was cancelled.
    #[must_use]
    pub fn cancelled_error(what: &str) -> SubError {
        SubError::new(codes::CANCELLED, format!("{what} was cancelled"))
    }
}

/// How urgent a job is relative to the others waiting.
///
/// The queue is strictly ordered: no [`Priority::Background`] job starts while
/// an [`Priority::Interactive`] one waits and a worker is free. Within one
/// priority, jobs start in submission order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Speculative work nobody is waiting for: proxies, whole-library scans.
    Background,
    /// The default: thumbnails and waveforms for media already imported.
    Normal,
    /// Work a person is watching happen: the strip of the clip under the
    /// pointer, the waveform of the sequence being scrubbed.
    Interactive,
}

impl Priority {
    /// The priority name as it appears in events and logs.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Normal => "normal",
            Self::Interactive => "interactive",
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Identifier of a submitted job, unique within one [`JobService`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(u64);

impl JobId {
    /// The identifier as a plain number, for logs and the MCP bridge.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "job-{}", self.0)
    }
}

/// Where a job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Submitted, waiting for a free worker.
    Queued,
    /// A worker is running it.
    Running,
    /// It ran to completion.
    Completed,
    /// It stopped with an error.
    Failed,
    /// It was cancelled before or during the run.
    Cancelled,
}

impl JobState {
    /// Whether the job will never change state again.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    /// The job returned `Ok`.
    Completed,
    /// The job returned an error other than cancellation.
    Failed(SubError),
    /// The job was cancelled, either before it started or while it ran.
    Cancelled,
}

impl JobOutcome {
    /// Whether the job ran to completion.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed)
    }

    /// Whether the job was cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// The error of a failed job.
    #[must_use]
    pub fn error(&self) -> Option<&SubError> {
        match self {
            Self::Failed(err) => Some(err),
            _ => None,
        }
    }

    /// The matching [`JobState`].
    #[must_use]
    pub fn state(&self) -> JobState {
        match self {
            Self::Completed => JobState::Completed,
            Self::Failed(_) => JobState::Failed,
            Self::Cancelled => JobState::Cancelled,
        }
    }

    /// Turns the outcome back into a result.
    ///
    /// # Errors
    ///
    /// Returns the job's error, or [`codes::CANCELLED`] when it was cancelled.
    pub fn into_result(self) -> SubResult<()> {
        match self {
            Self::Completed => Ok(()),
            Self::Failed(err) => Err(err),
            Self::Cancelled => Err(CancelToken::cancelled_error("the job")),
        }
    }
}

/// Something that happened to a job, delivered to every subscriber in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    /// The job was accepted into the queue.
    Queued {
        /// Which job.
        id: JobId,
        /// The job's kind, as given to [`JobService::submit`].
        kind: &'static str,
        /// The priority it was queued at.
        priority: Priority,
    },
    /// A worker picked the job up.
    Started {
        /// Which job.
        id: JobId,
        /// The job's kind.
        kind: &'static str,
    },
    /// The job reported progress. `total` is `0` while the job does not yet
    /// know how much work there is.
    Progress {
        /// Which job.
        id: JobId,
        /// The job's kind.
        kind: &'static str,
        /// Units of work finished.
        done: u64,
        /// Units of work expected, or `0` when unknown.
        total: u64,
    },
    /// The job reached a terminal state.
    Finished {
        /// Which job.
        id: JobId,
        /// The job's kind.
        kind: &'static str,
        /// How it ended.
        outcome: JobOutcome,
    },
}

impl JobEvent {
    /// The job this event is about.
    #[must_use]
    pub fn id(&self) -> JobId {
        match self {
            Self::Queued { id, .. }
            | Self::Started { id, .. }
            | Self::Progress { id, .. }
            | Self::Finished { id, .. } => *id,
        }
    }

    /// The kind of the job this event is about.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Queued { kind, .. }
            | Self::Started { kind, .. }
            | Self::Progress { kind, .. }
            | Self::Finished { kind, .. } => kind,
        }
    }
}

/// What a running job is given: its cancel flag and its progress channel.
pub struct JobContext {
    id: JobId,
    kind: &'static str,
    cancel: CancelToken,
    events: Arc<Subscribers>,
}

impl fmt::Debug for JobContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobContext")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("cancelled", &self.cancel.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl JobContext {
    /// The running job's identifier.
    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }

    /// The running job's kind.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// Whether cancellation has been asked for. Long jobs must check this
    /// between units of work and return [`JobContext::cancelled`] when set.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// The error a cancelled job should return.
    #[must_use]
    pub fn cancelled(&self) -> SubError {
        CancelToken::cancelled_error(self.kind)
    }

    /// `Err(cancelled)` when cancellation has been asked for, `Ok(())`
    /// otherwise, so a job body can write `ctx.check()?;` in its loop.
    ///
    /// # Errors
    ///
    /// Returns [`codes::CANCELLED`] when the job has been cancelled.
    pub fn check(&self) -> SubResult<()> {
        if self.is_cancelled() {
            return Err(self.cancelled());
        }
        Ok(())
    }

    /// The job's cancel token, to hand to work running deeper down.
    #[must_use]
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Publishes progress. `total` may be `0` while the size is unknown.
    pub fn progress(&self, done: u64, total: u64) {
        self.events.publish(&JobEvent::Progress {
            id: self.id,
            kind: self.kind,
            done,
            total,
        });
    }
}

/// A submitted job: its identity, its state, and the way to cancel or await it.
///
/// Dropping a handle does not cancel the job; it only gives up the ability to
/// wait for it.
#[derive(Debug, Clone)]
pub struct JobHandle {
    id: JobId,
    kind: &'static str,
    priority: Priority,
    cancel: CancelToken,
    slot: Arc<Slot>,
}

impl JobHandle {
    /// The job's identifier.
    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }

    /// The job's kind.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The priority it was submitted at.
    #[must_use]
    pub fn priority(&self) -> Priority {
        self.priority
    }

    /// Where the job is right now.
    #[must_use]
    pub fn state(&self) -> JobState {
        self.slot.state()
    }

    /// Whether the job has reached a terminal state.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.state().is_terminal()
    }

    /// Asks the job to stop. A job that has not started never runs; a running
    /// one sees [`JobContext::is_cancelled`] on its next check.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The job's cancel token.
    #[must_use]
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Blocks until the job reaches a terminal state and reports how it ended.
    #[must_use]
    pub fn wait(&self) -> JobOutcome {
        self.slot.wait()
    }
}

/// The shared cell a worker writes a job's outcome into.
#[derive(Debug)]
struct Slot {
    state: Mutex<(JobState, Option<JobOutcome>)>,
    finished: Condvar,
}

impl Slot {
    fn new() -> Self {
        Self {
            state: Mutex::new((JobState::Queued, None)),
            finished: Condvar::new(),
        }
    }

    fn state(&self) -> JobState {
        self.lock().0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (JobState, Option<JobOutcome>)> {
        self.state.lock().unwrap_or_else(|poisoned| {
            // A panicking job body is caught before it can leave this lock
            // poisoned, but a panic elsewhere must not deadlock the queue.
            poisoned.into_inner()
        })
    }

    fn set_running(&self) {
        let mut guard = self.lock();
        if guard.0 == JobState::Queued {
            guard.0 = JobState::Running;
        }
    }

    /// Records the outcome, unless the job already finished. Returns the
    /// outcome that was actually stored, so a caller only publishes once.
    fn finish(&self, outcome: JobOutcome) -> Option<JobOutcome> {
        let mut guard = self.lock();
        if guard.0.is_terminal() {
            return None;
        }
        guard.0 = outcome.state();
        guard.1 = Some(outcome.clone());
        drop(guard);
        self.finished.notify_all();
        Some(outcome)
    }

    fn wait(&self) -> JobOutcome {
        let mut guard = self.lock();
        while !guard.0.is_terminal() {
            guard = self
                .finished
                .wait(guard)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        guard.1.clone().unwrap_or(JobOutcome::Failed(SubError::new(
            codes::INTERNAL,
            "a job reached a terminal state without an outcome",
        )))
    }
}

/// The event fan-out: every subscriber gets every event, in order.
#[derive(Debug, Default)]
struct Subscribers {
    senders: Mutex<Vec<Sender<JobEvent>>>,
}

impl Subscribers {
    fn subscribe(&self) -> Receiver<JobEvent> {
        let (tx, rx) = channel();
        self.lock().push(tx);
        rx
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Sender<JobEvent>>> {
        self.senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn publish(&self, event: &JobEvent) {
        let mut senders = self.lock();
        // A subscriber that dropped its receiver is forgotten rather than
        // making every later publish pay for it.
        senders.retain(|tx| tx.send(event.clone()).is_ok());
    }
}

/// The body of a submitted job, boxed for the queue.
type JobBody = Box<dyn FnOnce(&JobContext) -> SubResult<()> + Send>;

/// One queued job, ordered by priority then submission sequence.
struct Queued {
    priority: Priority,
    sequence: u64,
    id: JobId,
    kind: &'static str,
    cancel: CancelToken,
    slot: Arc<Slot>,
    run: JobBody,
}

impl Queued {
    /// The ordering key: higher priority first, then lower sequence first.
    fn key(&self) -> (Priority, Reverse<u64>) {
        (self.priority, Reverse(self.sequence))
    }
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for Queued {}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

/// The state every worker and every handle shares.
struct Shared {
    queue: Mutex<Pending>,
    work_available: Condvar,
    events: Arc<Subscribers>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct Pending {
    heap: BinaryHeap<Queued>,
    shutting_down: bool,
    /// The cancel token of every job a worker is running right now, so
    /// [`JobService::cancel_all`] can reach the work already in flight.
    running: HashMap<JobId, CancelToken>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Takes the highest-priority job, or `None` once the service is shutting
    /// down and the queue has drained.
    fn take(&self) -> Option<Queued> {
        let mut pending = self.lock();
        loop {
            if let Some(job) = pending.heap.pop() {
                pending.running.insert(job.id, job.cancel.clone());
                return Some(job);
            }
            if pending.shutting_down {
                return None;
            }
            pending = self
                .work_available
                .wait(pending)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn finished_one(&self, id: JobId) {
        let mut pending = self.lock();
        pending.running.remove(&id);
        drop(pending);
        self.work_available.notify_all();
    }
}

/// A pool of worker threads running submitted jobs by priority.
///
/// The service shuts down when it is dropped: no more jobs are accepted, jobs
/// still queued are cancelled, running jobs are asked to stop, and the workers
/// are joined.
pub struct JobService {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl fmt::Debug for JobService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobService")
            .field("workers", &self.workers.len())
            .field("queued", &self.queued_len())
            .finish_non_exhaustive()
    }
}

impl JobService {
    /// Starts a service with `workers` threads (at least one).
    ///
    /// # Panics
    ///
    /// Panics if the operating system refuses to start a worker thread, which
    /// a process that cannot spawn threads cannot recover from anyway.
    #[must_use]
    pub fn new(workers: usize) -> Self {
        let workers = workers.max(1);
        let shared = Arc::new(Shared {
            queue: Mutex::new(Pending::default()),
            work_available: Condvar::new(),
            events: Arc::new(Subscribers::default()),
            next_id: AtomicU64::new(1),
        });
        let threads = (0..workers)
            .map(|index| {
                let shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("sub-jobs-{index}"))
                    .spawn(move || worker_loop(&shared))
                    .expect("a job worker thread must be spawnable")
            })
            .collect();
        Self {
            shared,
            workers: threads,
        }
    }

    /// Starts a service sized to the machine, leaving a core for the UI and
    /// one for the engine: `min(4, max(1, parallelism - 2))` workers.
    #[must_use]
    pub fn with_default_workers() -> Self {
        let parallelism = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        Self::new(parallelism.saturating_sub(2).clamp(1, 4))
    }

    /// A receiver of every event published from now on.
    ///
    /// Each subscriber has its own queue; dropping the receiver unsubscribes.
    #[must_use]
    pub fn subscribe(&self) -> Receiver<JobEvent> {
        self.shared.events.subscribe()
    }

    /// Queues `run` and returns its handle immediately.
    ///
    /// `kind` is a static name (`"thumbnails"`, `"waveform"`, `"proxy"`) that
    /// travels with every event about the job.
    pub fn submit<F>(&self, kind: &'static str, priority: Priority, run: F) -> JobHandle
    where
        F: FnOnce(&JobContext) -> SubResult<()> + Send + 'static,
    {
        let id = JobId(self.shared.next_id.fetch_add(1, Ordering::Relaxed));
        let cancel = CancelToken::new();
        let slot = Arc::new(Slot::new());
        let handle = JobHandle {
            id,
            kind,
            priority,
            cancel: cancel.clone(),
            slot: Arc::clone(&slot),
        };

        let mut pending = self.shared.lock();
        if pending.shutting_down {
            drop(pending);
            if let Some(outcome) = slot.finish(JobOutcome::Cancelled) {
                self.shared
                    .events
                    .publish(&JobEvent::Finished { id, kind, outcome });
            }
            return handle;
        }
        // The sequence is the id: both are handed out under one counter, in
        // submission order.
        pending.heap.push(Queued {
            priority,
            sequence: id.0,
            id,
            kind,
            cancel,
            slot,
            run: Box::new(run),
        });
        drop(pending);
        self.shared
            .events
            .publish(&JobEvent::Queued { id, kind, priority });
        self.shared.work_available.notify_one();
        handle
    }

    /// How many jobs are waiting for a worker.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.shared.lock().heap.len()
    }

    /// How many jobs a worker is running right now.
    #[must_use]
    pub fn running_len(&self) -> usize {
        self.shared.lock().running.len()
    }

    /// Whether nothing is queued and nothing is running.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        let pending = self.shared.lock();
        pending.heap.is_empty() && pending.running.is_empty()
    }

    /// Blocks until every submitted job has finished.
    pub fn wait_idle(&self) {
        let mut pending = self.shared.lock();
        while !pending.heap.is_empty() || !pending.running.is_empty() {
            pending = self
                .shared
                .work_available
                .wait(pending)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Cancels every queued and running job. The service stays usable.
    ///
    /// A queued job never runs; a running one is asked to stop through its
    /// token and stops at its next [`JobContext::check`].
    pub fn cancel_all(&self) {
        let mut pending = self.shared.lock();
        let queued = std::mem::take(&mut pending.heap);
        let running: Vec<CancelToken> = pending.running.values().cloned().collect();
        drop(pending);
        for token in running {
            token.cancel();
        }
        for job in queued {
            job.cancel.cancel();
            if let Some(outcome) = job.slot.finish(JobOutcome::Cancelled) {
                self.shared.events.publish(&JobEvent::Finished {
                    id: job.id,
                    kind: job.kind,
                    outcome,
                });
            }
        }
        self.shared.work_available.notify_all();
    }
}

impl Drop for JobService {
    fn drop(&mut self) {
        self.cancel_all();
        {
            let mut pending = self.shared.lock();
            pending.shutting_down = true;
        }
        self.shared.work_available.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// One worker: take the most urgent job, run it, publish what happened.
fn worker_loop(shared: &Arc<Shared>) {
    while let Some(job) = shared.take() {
        let Queued {
            id,
            kind,
            cancel,
            slot,
            run,
            ..
        } = job;

        let outcome = if cancel.is_cancelled() {
            JobOutcome::Cancelled
        } else {
            slot.set_running();
            shared.events.publish(&JobEvent::Started { id, kind });
            let ctx = JobContext {
                id,
                kind,
                cancel: cancel.clone(),
                events: Arc::clone(&shared.events),
            };
            // A panicking job must not take the worker — and the whole job
            // service — down with it; it becomes a core.internal failure.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&ctx)));
            match result {
                Ok(Ok(())) => JobOutcome::Completed,
                Ok(Err(err)) => {
                    if err.code == codes::CANCELLED || cancel.is_cancelled() {
                        JobOutcome::Cancelled
                    } else {
                        JobOutcome::Failed(err)
                    }
                }
                Err(panic) => JobOutcome::Failed(
                    SubError::new(codes::INTERNAL, format!("the {kind} job panicked"))
                        .with_detail("panic", panic_message(&panic)),
                ),
            }
        };

        if let Some(outcome) = slot.finish(outcome) {
            match &outcome {
                JobOutcome::Failed(err) => {
                    tracing::warn!(job = %id, kind, error = %err, "job failed");
                }
                JobOutcome::Cancelled => tracing::debug!(job = %id, kind, "job cancelled"),
                JobOutcome::Completed => tracing::debug!(job = %id, kind, "job completed"),
            }
            shared
                .events
                .publish(&JobEvent::Finished { id, kind, outcome });
        }
        shared.finished_one(id);
    }
}

/// The message a panic payload carries, when it is a string at all.
fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    use super::*;

    /// Waits for `condition` or fails the test; no test here sleeps a fixed
    /// time it does not need.
    fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn a_submitted_job_runs_and_reports_completion() {
        let service = JobService::new(1);
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let handle = service.submit("demo", Priority::Normal, move |_ctx| {
            flag.store(true, Ordering::Release);
            Ok(())
        });
        assert_eq!(handle.wait(), JobOutcome::Completed);
        assert!(ran.load(Ordering::Acquire));
        assert_eq!(handle.state(), JobState::Completed);
        assert!(handle.is_finished());
    }

    #[test]
    fn a_failing_job_keeps_its_error_code() {
        let service = JobService::new(1);
        let handle = service.submit("demo", Priority::Normal, |_ctx| {
            Err(SubError::new(codes::IO, "disk gone"))
        });
        let outcome = handle.wait();
        assert_eq!(handle.state(), JobState::Failed);
        assert_eq!(outcome.error().map(|e| e.code.clone()), Some(codes::IO));
        assert!(outcome.into_result().is_err());
    }

    #[test]
    fn a_panicking_job_becomes_an_internal_failure_and_the_worker_survives() {
        let service = JobService::new(1);
        let panicked = service.submit("demo", Priority::Normal, |_ctx| {
            panic!("boom");
        });
        assert_eq!(panicked.wait().state(), JobState::Failed);
        let err = panicked.wait();
        let err = err.error().expect("a failure");
        assert_eq!(err.code, codes::INTERNAL);
        assert_eq!(
            err.details.get("panic").and_then(serde_json::Value::as_str),
            Some("boom")
        );

        let after = service.submit("demo", Priority::Normal, |_ctx| Ok(()));
        assert!(after.wait().is_completed());
    }

    #[test]
    fn jobs_run_by_priority_then_submission_order() {
        let service = JobService::new(1);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let blocker_gate = Arc::clone(&gate);
        // One worker, held by a job that only returns when released, so the
        // ordering of everything queued behind it is deterministic.
        let blocker = service.submit("blocker", Priority::Interactive, move |_ctx| {
            let (lock, cvar) = &*blocker_gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cvar.wait(open).unwrap();
            }
            Ok(())
        });
        wait_for("the blocker to start", || {
            blocker.state() == JobState::Running
        });

        let order = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for (name, priority) in [
            ("bg-1", Priority::Background),
            ("normal-1", Priority::Normal),
            ("bg-2", Priority::Background),
            ("interactive", Priority::Interactive),
            ("normal-2", Priority::Normal),
        ] {
            let order = Arc::clone(&order);
            handles.push(service.submit(name, priority, move |ctx| {
                order.lock().unwrap().push(ctx.kind());
                Ok(())
            }));
        }
        wait_for("everything to queue", || service.queued_len() == 5);

        let (lock, cvar) = &*gate;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
        for handle in &handles {
            assert!(handle.wait().is_completed());
        }
        assert!(blocker.wait().is_completed());

        assert_eq!(
            *order.lock().unwrap(),
            vec!["interactive", "normal-1", "normal-2", "bg-1", "bg-2"]
        );
    }

    #[test]
    fn a_queued_job_that_is_cancelled_never_runs() {
        let service = JobService::new(1);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let blocker_gate = Arc::clone(&gate);
        let blocker = service.submit("blocker", Priority::Normal, move |_ctx| {
            let (lock, cvar) = &*blocker_gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cvar.wait(open).unwrap();
            }
            Ok(())
        });
        wait_for("the blocker to start", || {
            blocker.state() == JobState::Running
        });

        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let queued = service.submit("victim", Priority::Normal, move |_ctx| {
            flag.store(true, Ordering::Release);
            Ok(())
        });
        wait_for("the victim to queue", || service.queued_len() == 1);
        queued.cancel();

        let (lock, cvar) = &*gate;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
        assert!(blocker.wait().is_completed());
        assert!(queued.wait().is_cancelled());
        assert_eq!(queued.state(), JobState::Cancelled);
        assert!(!ran.load(Ordering::Acquire), "a cancelled job must not run");
    }

    #[test]
    fn a_running_job_sees_its_cancellation_and_reports_it_as_cancelled() {
        let service = JobService::new(1);
        let started = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&started);
        let handle = service.submit("long", Priority::Normal, move |ctx| {
            flag.store(true, Ordering::Release);
            loop {
                ctx.check()?;
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        wait_for("the job to start", || started.load(Ordering::Acquire));
        handle.cancel();
        assert!(handle.wait().is_cancelled());
        assert_eq!(handle.state(), JobState::Cancelled);
    }

    #[test]
    fn progress_and_lifecycle_events_reach_every_subscriber_in_order() {
        let service = JobService::new(1);
        let first = service.subscribe();
        let second = service.subscribe();
        let handle = service.submit("thumbnails", Priority::Interactive, |ctx| {
            ctx.progress(1, 2);
            ctx.progress(2, 2);
            Ok(())
        });
        assert!(handle.wait().is_completed());

        for events in [first, second] {
            let seen: Vec<JobEvent> = events.try_iter().collect();
            assert_eq!(
                seen,
                vec![
                    JobEvent::Queued {
                        id: handle.id(),
                        kind: "thumbnails",
                        priority: Priority::Interactive,
                    },
                    JobEvent::Started {
                        id: handle.id(),
                        kind: "thumbnails",
                    },
                    JobEvent::Progress {
                        id: handle.id(),
                        kind: "thumbnails",
                        done: 1,
                        total: 2,
                    },
                    JobEvent::Progress {
                        id: handle.id(),
                        kind: "thumbnails",
                        done: 2,
                        total: 2,
                    },
                    JobEvent::Finished {
                        id: handle.id(),
                        kind: "thumbnails",
                        outcome: JobOutcome::Completed,
                    },
                ]
            );
            assert_eq!(seen[0].id(), handle.id());
            assert_eq!(seen[0].kind(), "thumbnails");
        }
    }

    #[test]
    fn several_workers_run_jobs_at_once_and_the_service_reports_being_idle() {
        let service = JobService::new(4);
        let done = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let done = Arc::clone(&done);
                service.submit("counter", Priority::Normal, move |_ctx| {
                    done.fetch_add(1, Ordering::Release);
                    Ok(())
                })
            })
            .collect();
        service.wait_idle();
        assert!(service.is_idle());
        assert_eq!(done.load(Ordering::Acquire), 16);
        assert!(handles.iter().all(JobHandle::is_finished));
        assert_eq!(service.queued_len(), 0);
        assert_eq!(service.running_len(), 0);
    }

    #[test]
    fn cancel_all_empties_the_queue() {
        let service = JobService::new(1);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let blocker_gate = Arc::clone(&gate);
        let blocker = service.submit("blocker", Priority::Normal, move |ctx| {
            let (lock, cvar) = &*blocker_gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                let (next, timeout) = cvar.wait_timeout(open, Duration::from_millis(5)).unwrap();
                open = next;
                if timeout.timed_out() {
                    ctx.check()?;
                }
            }
            Ok(())
        });
        wait_for("the blocker to start", || {
            blocker.state() == JobState::Running
        });
        let queued: Vec<_> = (0..3)
            .map(|_| service.submit("victim", Priority::Normal, |_ctx| Ok(())))
            .collect();
        wait_for("the victims to queue", || service.queued_len() == 3);

        service.cancel_all();
        assert_eq!(service.queued_len(), 0);
        for handle in &queued {
            assert!(handle.wait().is_cancelled());
        }
        assert!(blocker.wait().is_cancelled());
    }

    #[test]
    fn dropping_the_service_cancels_outstanding_work() {
        let service = JobService::new(1);
        let handle = service.submit("long", Priority::Normal, |ctx| {
            loop {
                ctx.check()?;
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        wait_for("the job to start", || handle.state() == JobState::Running);
        drop(service);
        assert!(handle.wait().is_cancelled());
    }

    #[test]
    fn priorities_and_states_have_the_expected_order_and_names() {
        assert!(Priority::Interactive > Priority::Normal);
        assert!(Priority::Normal > Priority::Background);
        assert_eq!(Priority::Background.to_string(), "background");
        assert!(!JobState::Queued.is_terminal());
        assert!(!JobState::Running.is_terminal());
        for state in [JobState::Completed, JobState::Failed, JobState::Cancelled] {
            assert!(state.is_terminal());
        }
        assert_eq!(
            serde_json::to_string(&Priority::Interactive).unwrap(),
            "\"interactive\""
        );
        assert_eq!(JobId(7).to_string(), "job-7");
        assert_eq!(JobId(7).get(), 7);
    }

    #[test]
    fn a_cancel_token_is_shared_by_its_clones() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
        assert_eq!(
            CancelToken::cancelled_error("the index build").code,
            codes::CANCELLED
        );
    }
}
