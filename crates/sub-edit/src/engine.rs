//! The engine thread: the single owner of the project state.
//!
//! Several clients mutate one project — the UI, the Command API, the MCP
//! bridge, plugins — and the UI thread must never block on any of them, so one
//! thread owns the [`Project`] and its [`History`] and serialises every
//! mutation through a queue (docs/PLAN.md §4).
//!
//! - **Writes** are commands sent to [`EngineHandle`], which is `Clone` and
//!   `Send`: any thread can submit one. Submitting never blocks the engine;
//!   the reply travels back on a one-slot channel.
//! - **Reads** are immutable snapshots. [`EngineHandle::snapshot`] hands out an
//!   `Arc<Project>` cloned out of a slot the engine swaps after each mutation,
//!   so a reader locks only long enough to bump a refcount and then holds a
//!   consistent project for as long as it likes — never a lock across a frame.
//! - **Changes** are broadcast as [`ChangeEvent`]s on an [`EventBus`]. Every
//!   applied, undone and redone command emits one, and a subscriber that stops
//!   draining loses its oldest events instead of stalling the engine.
//!
//! ```
//! use sub_edit::{ChangeType, Engine};
//! use sub_edit::commands::AddTrack;
//! use sub_model::{Project, Sequence, SequenceSettings, TrackKind};
//!
//! let mut project = Project::new("Doc cut");
//! let sequence = Sequence::new("Main", SequenceSettings::default());
//! let sequence_id = sequence.id;
//! project.sequences.push(sequence);
//!
//! let engine = Engine::spawn(project).unwrap();
//! let events = engine.handle().subscribe();
//!
//! let applied = engine
//!     .handle()
//!     .apply(AddTrack {
//!         sequence: sequence_id,
//!         name: "V1".to_owned(),
//!         kind: TrackKind::Video,
//!         index: None,
//!     })
//!     .unwrap();
//! assert_eq!(applied.revision, 1);
//! assert_eq!(events.recv().unwrap().change, ChangeType::Added);
//! assert_eq!(engine.handle().snapshot().sequences[0].tracks.len(), 1);
//!
//! engine.handle().undo().unwrap();
//! assert!(engine.handle().snapshot().sequences[0].tracks.is_empty());
//! engine.shutdown().unwrap();
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sub_core::{SubError, SubResult};
use sub_model::{Project, SequenceId};
use sub_time::{Rational, RationalTime, TimeRange};

use crate::bus::{DEFAULT_EVENT_CAPACITY, EventBus, EventReceiver};
use crate::codes;
use crate::command::{BoxedCommand, Command, CommandEnvelope, CommandRegistry};
use crate::commands;
use crate::event::{ChangeEvent, ChangeOrigin};
use crate::history::{DEFAULT_DEPTH, History};
use crate::playback::{PlaybackScheduler, PlayheadEvent, ShuttleSpeed};

/// How the engine thread is set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    /// How many undo steps the history keeps.
    pub history_depth: usize,
    /// How many events one subscriber may fall behind before it starts losing
    /// the oldest ones.
    pub event_capacity: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            history_depth: DEFAULT_DEPTH,
            event_capacity: DEFAULT_EVENT_CAPACITY,
        }
    }
}

/// What one successful engine operation did.
#[derive(Debug, Clone)]
pub struct Applied {
    /// The revision the project reached.
    pub revision: u64,
    /// The label of the history step, as the undo menu shows it.
    pub label: String,
    /// The changes broadcast for this operation, in the order they happened.
    pub events: Vec<ChangeEvent>,
    /// The project as it is now.
    pub project: Arc<Project>,
}

/// The state of the undo and redo stacks, as a history panel reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySummary {
    /// Whether there is a step to undo.
    pub can_undo: bool,
    /// Whether there is a step to redo.
    pub can_redo: bool,
    /// The label of the step an undo would reverse.
    pub undo_label: Option<String>,
    /// The label of the step a redo would replay.
    pub redo_label: Option<String>,
    /// The number of steps that can be undone.
    pub undo_len: usize,
    /// The number of steps that can be redone.
    pub redo_len: usize,
    /// Whether a command group is open.
    pub in_group: bool,
}

/// One transport operation, as submitted to the engine.
///
/// Playback is engine state rather than project state, so these are requests
/// on the queue instead of commands: nothing here is undoable and nothing here
/// changes the project (docs/PLAN.md §5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackOp {
    /// L: play forward, shuttling faster on every repeat.
    PlayForward,
    /// J: play backwards, shuttling faster on every repeat.
    PlayBackward,
    /// K: stop where the playhead stands.
    Pause,
    /// Space: play at 1x forward, or pause if anything is playing.
    Toggle,
    /// Run at exactly this speed.
    SetSpeed(ShuttleSpeed),
    /// Move the playhead, without stopping playback.
    Seek(RationalTime),
    /// Set or clear the loop range.
    SetLoopRange(Option<TimeRange>),
    /// Set the timebase and the length the clock runs over.
    SetTimebase {
        /// The sequence timebase.
        rate: Rational,
        /// How long the sequence is.
        duration: RationalTime,
    },
    /// Take the timebase and the length from a sequence of the project.
    FollowSequence(SequenceId),
    /// Ask for the transport state without changing it.
    Status,
}

/// Where the transport is, as a caller or a transport bar reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackStatus {
    /// Where the playhead is.
    pub position: RationalTime,
    /// How long the sequence being played is.
    pub duration: RationalTime,
    /// The shuttle speed.
    pub speed: ShuttleSpeed,
    /// Whether playback is running.
    pub playing: bool,
    /// The loop range in force, if any.
    pub loop_range: Option<TimeRange>,
    /// Frames dropped in this run of playback.
    pub dropped_frames: u64,
}

/// The clock a new engine starts with: the first sequence of the project, or
/// an empty one at 24 fps when the project has none yet.
fn initial_scheduler(project: &Project) -> PlaybackScheduler {
    project.sequences.first().map_or_else(
        || PlaybackScheduler::new(Rational::FPS_24),
        PlaybackScheduler::for_sequence,
    )
}

/// The reply channel of one request.
type Reply<T> = SyncSender<SubResult<T>>;

/// One unit of work for the engine thread.
#[derive(Debug)]
enum Request {
    Apply {
        command: BoxedCommand,
        reply: Reply<Applied>,
    },
    ApplyEnvelope {
        envelope: CommandEnvelope,
        reply: Reply<Applied>,
    },
    Undo {
        reply: Reply<Option<Applied>>,
    },
    Redo {
        reply: Reply<Option<Applied>>,
    },
    BeginGroup {
        label: String,
        reply: Reply<()>,
    },
    CommitGroup {
        reply: Reply<bool>,
    },
    AbortGroup {
        reply: Reply<()>,
    },
    History {
        reply: Reply<HistorySummary>,
    },
    Playback {
        op: PlaybackOp,
        reply: Reply<PlaybackStatus>,
    },
    Shutdown {
        reply: Reply<()>,
    },
}

/// What the engine thread publishes and every handle can read without waiting
/// for the queue.
#[derive(Debug)]
struct Published {
    snapshot: Mutex<Arc<Project>>,
    revision: AtomicU64,
    bus: EventBus,
    playhead: EventBus<PlayheadEvent>,
}

impl Published {
    /// Stores a new snapshot and the revision it belongs to. The snapshot is
    /// written first so a reader that saw revision `n` always finds a project
    /// at least that new.
    fn store(&self, project: &Arc<Project>, revision: u64) {
        let mut slot = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        *slot = Arc::clone(project);
        drop(slot);
        self.revision.store(revision, Ordering::Release);
    }
}

/// The engine thread and the handle to it.
///
/// Dropping the engine asks the thread to stop and joins it, so an `Engine`
/// held by the application owns the thread's lifetime. Clone
/// [`Engine::handle`] to talk to it from elsewhere.
#[derive(Debug)]
pub struct Engine {
    handle: EngineHandle,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    /// Starts an engine owning `project`, with the built-in command set and
    /// the default configuration.
    ///
    /// # Errors
    ///
    /// Returns whatever building the built-in registry or the configuration
    /// returns.
    pub fn spawn(project: Project) -> SubResult<Self> {
        Self::with_config(project, EngineConfig::default())
    }

    /// Starts an engine with an explicit configuration and the built-in
    /// command set.
    ///
    /// # Errors
    ///
    /// - `core.invalid_argument` when the configuration is out of range.
    /// - `edit.duplicate_command` if the built-in registry cannot be built,
    ///   which would be a bug in this crate.
    pub fn with_config(project: Project, config: EngineConfig) -> SubResult<Self> {
        Self::with_registry(project, config, commands::builtin_registry()?)
    }

    /// Starts an engine that also decodes the command kinds in `registry`,
    /// which is how plugin-contributed commands reach it.
    ///
    /// # Errors
    ///
    /// Returns `core.invalid_argument` when the configuration is out of range.
    pub fn with_registry(
        project: Project,
        config: EngineConfig,
        registry: CommandRegistry,
    ) -> SubResult<Self> {
        let history = History::with_depth(config.history_depth)?;
        let project = Arc::new(project);
        let published = Arc::new(Published {
            snapshot: Mutex::new(Arc::clone(&project)),
            revision: AtomicU64::new(0),
            bus: EventBus::new(config.event_capacity)?,
            playhead: EventBus::new(config.event_capacity)?,
        });

        let (sender, receiver) = mpsc::channel();
        let thread_published = Arc::clone(&published);
        let thread = thread::Builder::new()
            .name("sub-engine".to_owned())
            .spawn(move || {
                EngineThread {
                    scheduler: initial_scheduler(&project),
                    project,
                    history,
                    registry,
                    published: thread_published,
                    group_events: Vec::new(),
                    last_tick: None,
                }
                .run(&receiver);
            })
            .map_err(|err| {
                SubError::wrap(
                    sub_core::codes::INTERNAL,
                    "could not start the engine thread",
                    &err,
                )
            })?;

        Ok(Self {
            handle: EngineHandle {
                requests: sender,
                published,
            },
            thread: Some(thread),
        })
    }

    /// The handle used to submit commands and read snapshots. Clone it to give
    /// another thread access.
    #[must_use]
    pub fn handle(&self) -> &EngineHandle {
        &self.handle
    }

    /// Stops the engine thread and waits for it, after the commands already
    /// queued have been applied.
    ///
    /// # Errors
    ///
    /// Returns `core.internal` when the engine thread panicked.
    pub fn shutdown(mut self) -> SubResult<()> {
        self.stop()
    }

    /// The body of both [`Engine::shutdown`] and the `Drop` implementation.
    fn stop(&mut self) -> SubResult<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        let _ = self.handle.request(|reply| Request::Shutdown { reply });
        thread.join().map_err(|_| {
            SubError::new(
                sub_core::codes::INTERNAL,
                "the engine thread panicked; the project state is lost",
            )
        })
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// A cloneable, thread-safe handle to a running [`Engine`].
///
/// Every method that mutates blocks until the engine has applied the command
/// and returns its result; reading a snapshot or subscribing does not touch the
/// queue at all.
#[derive(Debug, Clone)]
pub struct EngineHandle {
    requests: mpsc::Sender<Request>,
    published: Arc<Published>,
}

impl EngineHandle {
    /// The project as of the last applied command.
    ///
    /// The returned `Arc` is an immutable snapshot: later commands build new
    /// ones and leave this one alone, so a renderer or a panel can hold it for
    /// a whole frame without blocking any writer.
    #[must_use]
    pub fn snapshot(&self) -> Arc<Project> {
        let slot = self
            .published
            .snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Arc::clone(&slot)
    }

    /// The revision of the last applied command; zero before the first one.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.published.revision.load(Ordering::Acquire)
    }

    /// Subscribes to change events. Events published before this call are not
    /// delivered; read [`EngineHandle::snapshot`] for the state so far.
    #[must_use]
    pub fn subscribe(&self) -> EventReceiver {
        self.published.bus.subscribe()
    }

    /// The number of live subscribers.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.published.bus.subscriber_count()
    }

    /// Applies a command and returns what it did.
    ///
    /// # Errors
    ///
    /// Whatever the command returns, or `edit.engine_stopped` when the engine
    /// thread is gone.
    pub fn apply<C: Command>(&self, command: C) -> SubResult<Applied> {
        self.apply_boxed(Box::new(command))
    }

    /// Applies an already-boxed command.
    ///
    /// # Errors
    ///
    /// The same as [`EngineHandle::apply`].
    pub fn apply_boxed(&self, command: BoxedCommand) -> SubResult<Applied> {
        self.request(|reply| Request::Apply { command, reply })
    }

    /// Decodes an envelope with the engine's registry and applies it: the path
    /// the Command API, the MCP bridge and plugins take.
    ///
    /// # Errors
    ///
    /// `edit.unknown_command`, `edit.invalid_command`, whatever the command
    /// returns, or `edit.engine_stopped`.
    pub fn apply_envelope(&self, envelope: CommandEnvelope) -> SubResult<Applied> {
        self.request(|reply| Request::ApplyEnvelope { envelope, reply })
    }

    /// Undoes the most recent history step, or returns `None` when there is
    /// nothing to undo.
    ///
    /// # Errors
    ///
    /// `edit.group_open` when a group is open, `core.internal` when an inverse
    /// fails to apply, or `edit.engine_stopped`.
    pub fn undo(&self) -> SubResult<Option<Applied>> {
        self.request(|reply| Request::Undo { reply })
    }

    /// Redoes the most recently undone step, or returns `None` when there is
    /// nothing to redo.
    ///
    /// # Errors
    ///
    /// The same as [`EngineHandle::undo`].
    pub fn redo(&self) -> SubResult<Option<Applied>> {
        self.request(|reply| Request::Redo { reply })
    }

    /// Starts a command group: every command applied until
    /// [`EngineHandle::commit_group`] becomes one undo step.
    ///
    /// Groups belong to the engine, not to the caller: a second client that
    /// opens one while another is open gets `edit.group_open`.
    ///
    /// # Errors
    ///
    /// `edit.group_open`, or `edit.engine_stopped`.
    pub fn begin_group(&self, label: impl Into<String>) -> SubResult<()> {
        let label = label.into();
        self.request(|reply| Request::BeginGroup { label, reply })
    }

    /// Closes the open group as one undo step, returning `false` when it
    /// applied no commands.
    ///
    /// # Errors
    ///
    /// `edit.no_group`, or `edit.engine_stopped`.
    pub fn commit_group(&self) -> SubResult<bool> {
        self.request(|reply| Request::CommitGroup { reply })
    }

    /// Closes the open group and undoes everything it applied, broadcasting the
    /// reversal.
    ///
    /// # Errors
    ///
    /// `edit.no_group`, `core.internal` when an inverse fails, or
    /// `edit.engine_stopped`.
    pub fn abort_group(&self) -> SubResult<()> {
        self.request(|reply| Request::AbortGroup { reply })
    }

    /// The state of the undo and redo stacks.
    ///
    /// # Errors
    ///
    /// Returns `edit.engine_stopped` when the engine thread is gone.
    pub fn history(&self) -> SubResult<HistorySummary> {
        self.request(|reply| Request::History { reply })
    }

    /// Sends one request and waits for its reply.
    /// Subscribes to the playhead.
    ///
    /// This is a separate stream from [`EngineHandle::subscribe`]: playback
    /// moves the playhead many times a second and none of those moves is a
    /// project change, so an editing subscriber never has to filter them out.
    #[must_use]
    pub fn subscribe_playhead(&self) -> EventReceiver<PlayheadEvent> {
        self.published.playhead.subscribe()
    }

    /// Runs one transport operation and returns the transport state.
    ///
    /// # Errors
    ///
    /// - `edit.engine_stopped` when the engine thread is gone.
    /// - `edit.invalid_time` when a loop range covers no frames.
    /// - `edit.sequence_not_found` when following a sequence the project does
    ///   not hold.
    pub fn playback(&self, op: PlaybackOp) -> SubResult<PlaybackStatus> {
        self.request(|reply| Request::Playback { op, reply })
    }

    /// L: play forward, shuttling faster on every repeat.
    ///
    /// # Errors
    ///
    /// Returns `edit.engine_stopped` when the engine thread is gone.
    pub fn play_forward(&self) -> SubResult<PlaybackStatus> {
        self.playback(PlaybackOp::PlayForward)
    }

    /// J: play backwards, shuttling faster on every repeat.
    ///
    /// # Errors
    ///
    /// Returns `edit.engine_stopped` when the engine thread is gone.
    pub fn play_backward(&self) -> SubResult<PlaybackStatus> {
        self.playback(PlaybackOp::PlayBackward)
    }

    /// K: stop where the playhead stands.
    ///
    /// # Errors
    ///
    /// Returns `edit.engine_stopped` when the engine thread is gone.
    pub fn pause_playback(&self) -> SubResult<PlaybackStatus> {
        self.playback(PlaybackOp::Pause)
    }

    /// Space: play at 1x forward, or pause if anything is playing.
    ///
    /// # Errors
    ///
    /// Returns `edit.engine_stopped` when the engine thread is gone.
    pub fn toggle_playback(&self) -> SubResult<PlaybackStatus> {
        self.playback(PlaybackOp::Toggle)
    }

    /// Moves the playhead without stopping playback.
    ///
    /// # Errors
    ///
    /// Returns `edit.engine_stopped` when the engine thread is gone.
    pub fn seek(&self, position: RationalTime) -> SubResult<PlaybackStatus> {
        self.playback(PlaybackOp::Seek(position))
    }

    /// Sets or clears the loop range playback wraps around.
    ///
    /// # Errors
    ///
    /// - `edit.invalid_time` when the range covers no frames.
    /// - `edit.engine_stopped` when the engine thread is gone.
    pub fn set_loop_range(&self, range: Option<TimeRange>) -> SubResult<PlaybackStatus> {
        self.playback(PlaybackOp::SetLoopRange(range))
    }

    /// Points the clock at a sequence of the project, taking its timebase and
    /// its length.
    ///
    /// # Errors
    ///
    /// - `edit.sequence_not_found` when the project holds no such sequence.
    /// - `edit.engine_stopped` when the engine thread is gone.
    pub fn follow_sequence(&self, sequence: SequenceId) -> SubResult<PlaybackStatus> {
        self.playback(PlaybackOp::FollowSequence(sequence))
    }

    /// Where the transport is, without changing it.
    ///
    /// # Errors
    ///
    /// Returns `edit.engine_stopped` when the engine thread is gone.
    pub fn playback_status(&self) -> SubResult<PlaybackStatus> {
        self.playback(PlaybackOp::Status)
    }

    fn request<T>(&self, build: impl FnOnce(Reply<T>) -> Request) -> SubResult<T> {
        let (reply, replies) = sync_channel(1);
        self.requests.send(build(reply)).map_err(|_| stopped())?;
        replies.recv().map_err(|_| stopped())?
    }
}

/// The error every handle method returns once the engine thread is gone.
fn stopped() -> SubError {
    SubError::new(
        codes::ENGINE_STOPPED,
        "the engine thread is no longer running",
    )
}

/// The state the engine thread owns.
struct EngineThread {
    project: Arc<Project>,
    history: History,
    registry: CommandRegistry,
    published: Arc<Published>,
    group_events: Vec<ChangeEvent>,
    /// The playback clock. It is engine state but not project state: the
    /// playhead is where the user is looking, so it is not a command and is
    /// never undone.
    scheduler: PlaybackScheduler,
    /// When the clock was last advanced, so a wake-up knows how much wall
    /// time it has to account for.
    last_tick: Option<Instant>,
}

impl EngineThread {
    /// Serves requests until a shutdown or until every handle is dropped, then
    /// closes the event bus so subscribers stop waiting.
    fn run(mut self, requests: &Receiver<Request>) {
        loop {
            // While playing, the thread waits only until the next frame is
            // due; the tick that follows accounts for however long it really
            // waited, so a late wake-up drops frames instead of slowing the
            // clock down.
            let request = match self.scheduler.time_until_next_frame() {
                Some(wait) => match requests.recv_timeout(wait) {
                    Ok(request) => Some(request),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match requests.recv() {
                    Ok(request) => Some(request),
                    Err(mpsc::RecvError) => break,
                },
            };
            self.tick();
            if let Some(request) = request
                && self.serve(request)
            {
                break;
            }
        }
        self.published.bus.close();
        self.published.playhead.close();
    }

    /// Advances the playback clock by the wall time since the last wake-up and
    /// publishes the playhead when it moved.
    fn tick(&mut self) {
        let now = Instant::now();
        let previous = self.last_tick.replace(now);
        if !self.scheduler.is_playing() {
            return;
        }
        let elapsed = previous.map_or(Duration::ZERO, |then| now.saturating_duration_since(then));
        if let Some(tick) = self.scheduler.advance(elapsed) {
            self.publish_playhead(tick.wrapped);
        }
    }

    /// Applies one transport operation and reports the transport state.
    fn playback(&mut self, op: PlaybackOp) -> SubResult<PlaybackStatus> {
        match op {
            PlaybackOp::Status => return Ok(self.playback_status()),
            PlaybackOp::PlayForward => self.scheduler.play_forward(),
            PlaybackOp::PlayBackward => self.scheduler.play_backward(),
            PlaybackOp::Pause => self.scheduler.pause(),
            PlaybackOp::Toggle => self.scheduler.toggle(),
            PlaybackOp::SetSpeed(speed) => self.scheduler.set_speed(speed),
            PlaybackOp::Seek(position) => self.scheduler.seek(position),
            PlaybackOp::SetLoopRange(range) => self.scheduler.set_loop_range(range)?,
            PlaybackOp::SetTimebase { rate, duration } => {
                self.scheduler.set_rate(rate);
                self.scheduler.set_duration(duration);
            }
            PlaybackOp::FollowSequence(id) => {
                let sequence = self
                    .project
                    .sequences
                    .iter()
                    .find(|sequence| sequence.id == id)
                    .ok_or_else(|| {
                        SubError::new(codes::SEQUENCE_NOT_FOUND, "no such sequence")
                            .with_detail("sequence", id.to_string())
                    })?;
                self.scheduler.follow_sequence(sequence);
            }
        }
        // The clock starts from this moment, not from whenever the thread
        // last woke, so pressing play does not immediately drop frames.
        self.last_tick = Some(Instant::now());
        self.publish_playhead(false);
        Ok(self.playback_status())
    }

    /// The transport state, as a caller reads it back.
    fn playback_status(&self) -> PlaybackStatus {
        PlaybackStatus {
            position: self.scheduler.position(),
            duration: self.scheduler.duration(),
            speed: self.scheduler.speed(),
            playing: self.scheduler.is_playing(),
            loop_range: self.scheduler.loop_range(),
            dropped_frames: self.scheduler.dropped_frames(),
        }
    }

    /// Broadcasts where the playhead is now.
    fn publish_playhead(&self, wrapped: bool) {
        self.published
            .playhead
            .publish([self.scheduler.event(wrapped)]);
    }

    /// Handles one request, returning true when the engine should stop.
    fn serve(&mut self, request: Request) -> bool {
        match request {
            Request::Apply { command, reply } => {
                send(&reply, self.apply(command));
            }
            Request::ApplyEnvelope { envelope, reply } => {
                let result = self
                    .registry
                    .decode(&envelope)
                    .and_then(|command| self.apply(command));
                send(&reply, result);
            }
            Request::Undo { reply } => {
                let result = self.undo();
                send(&reply, result);
            }
            Request::Redo { reply } => {
                let result = self.redo();
                send(&reply, result);
            }
            Request::BeginGroup { label, reply } => {
                let result = self.history.begin_group(label).inspect(|()| {
                    self.group_events.clear();
                });
                send(&reply, result);
            }
            Request::CommitGroup { reply } => {
                let result = self.history.commit_group().inspect(|_| {
                    self.group_events.clear();
                });
                send(&reply, result);
            }
            Request::AbortGroup { reply } => {
                let result = self.abort_group();
                send(&reply, result);
            }
            Request::History { reply } => {
                let summary = self.summary();
                send(&reply, Ok(summary));
            }
            Request::Playback { op, reply } => {
                let result = self.playback(op);
                send(&reply, result);
            }
            Request::Shutdown { reply } => {
                send(&reply, Ok(()));
                return true;
            }
        }
        false
    }

    /// Applies one command, publishing the snapshot and the event it produced.
    fn apply(&mut self, command: BoxedCommand) -> SubResult<Applied> {
        let envelope = command.to_envelope()?;
        let label = command.label_erased();
        let in_group = self.history.in_group();
        let project = Arc::make_mut(&mut self.project);
        if let Err(err) = self.history.apply_boxed(project, command) {
            // A command that fails inside a group takes the whole group down
            // with it, so the changes already broadcast for it are reversed.
            if in_group && !self.history.in_group() {
                self.publish_group_rollback();
            }
            return Err(err);
        }

        let revision = self.next_revision();
        let events = vec![ChangeEvent::from_envelope(
            revision,
            &envelope,
            ChangeOrigin::Apply,
        )];
        if self.history.in_group() {
            self.group_events.extend(events.iter().cloned());
        }
        Ok(self.commit(revision, label, events))
    }

    /// Undoes one history step, publishing the reversal of every command it
    /// applied, newest first.
    fn undo(&mut self) -> SubResult<Option<Applied>> {
        let Some(envelopes) = self
            .history
            .undo_entries()
            .next_back()
            .map(crate::history::HistoryEntry::to_envelopes)
            .transpose()?
        else {
            return Ok(None);
        };

        let project = Arc::make_mut(&mut self.project);
        let Some(label) = self.history.undo(project)? else {
            return Ok(None);
        };

        let revision = self.next_revision();
        let events = envelopes
            .iter()
            .rev()
            .map(|envelope| {
                ChangeEvent::from_envelope(revision, envelope, ChangeOrigin::Undo).inverted()
            })
            .collect();
        Ok(Some(self.commit(revision, label, events)))
    }

    /// Redoes one history step, publishing the same changes the commands made
    /// the first time.
    fn redo(&mut self) -> SubResult<Option<Applied>> {
        let Some(envelopes) = self
            .history
            .redo_entries()
            .next()
            .map(crate::history::HistoryEntry::to_envelopes)
            .transpose()?
        else {
            return Ok(None);
        };

        let project = Arc::make_mut(&mut self.project);
        let Some(label) = self.history.redo(project)? else {
            return Ok(None);
        };

        let revision = self.next_revision();
        let events = envelopes
            .iter()
            .map(|envelope| ChangeEvent::from_envelope(revision, envelope, ChangeOrigin::Redo))
            .collect();
        Ok(Some(self.commit(revision, label, events)))
    }

    /// Aborts the open group and broadcasts the reversal of what it applied.
    ///
    /// The history cannot say what it rolled back, so the engine replays the
    /// events it emitted for the group's commands, newest first and inverted.
    fn abort_group(&mut self) -> SubResult<()> {
        let project = Arc::make_mut(&mut self.project);
        self.history.abort_group(project)?;
        self.publish_group_rollback();
        Ok(())
    }

    /// Publishes the reversal of every event the open group emitted, newest
    /// first, and empties the record of them.
    fn publish_group_rollback(&mut self) {
        let revision = self.next_revision();
        let events: Vec<ChangeEvent> = self
            .group_events
            .drain(..)
            .rev()
            .map(|event| {
                let mut event = event.inverted();
                event.revision = revision;
                event.origin = ChangeOrigin::Undo;
                event
            })
            .collect();
        self.commit(revision, "Abort group".to_owned(), events);
    }

    /// The next revision number. Revisions start at one, so zero means "no
    /// command has been applied yet".
    fn next_revision(&self) -> u64 {
        self.published.revision.load(Ordering::Acquire) + 1
    }

    /// Publishes the new snapshot and the events of one operation.
    fn commit(&self, revision: u64, label: String, events: Vec<ChangeEvent>) -> Applied {
        self.published.store(&self.project, revision);
        self.published.bus.publish(events.clone());
        Applied {
            revision,
            label,
            events,
            project: Arc::clone(&self.project),
        }
    }

    /// The history state a panel reads.
    fn summary(&self) -> HistorySummary {
        HistorySummary {
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            undo_label: self.history.undo_label().map(ToOwned::to_owned),
            redo_label: self.history.redo_label().map(ToOwned::to_owned),
            undo_len: self.history.undo_len(),
            redo_len: self.history.redo_len(),
            in_group: self.history.in_group(),
        }
    }
}

/// Sends a reply, ignoring a caller that stopped waiting.
fn send<T>(reply: &Reply<T>, result: SubResult<T>) {
    let _ = reply.send(result);
}

#[cfg(test)]
mod tests {
    use sub_model::{Project, Sequence, SequenceId, SequenceSettings, TrackKind};

    use super::*;
    use crate::commands::{AddTrack, RemoveTrack, RenameTrack};
    use crate::event::{ChangeType, EntityKind};

    fn fixture() -> (Project, SequenceId) {
        let mut project = Project::new("Doc cut");
        let sequence = Sequence::new("Main", SequenceSettings::default());
        let id = sequence.id;
        project.sequences.push(sequence);
        (project, id)
    }

    fn add_track(sequence: SequenceId, name: &str) -> AddTrack {
        AddTrack {
            sequence,
            name: name.to_owned(),
            kind: TrackKind::Video,
            index: None,
        }
    }

    #[test]
    fn the_transport_shuttles_and_reports_where_it_is() {
        let (project, sequence) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let handle = engine.handle();
        // Following the sequence takes its timebase; it is empty, so the
        // length is set explicitly afterwards.
        handle.follow_sequence(sequence).unwrap();
        handle
            .playback(PlaybackOp::SetTimebase {
                rate: Rational::FPS_24,
                duration: RationalTime::new(240, Rational::FPS_24),
            })
            .unwrap();

        let status = handle.play_forward().unwrap();
        assert_eq!(status.speed, ShuttleSpeed::Forward1x);
        assert!(status.playing);
        assert_eq!(
            handle.play_forward().unwrap().speed,
            ShuttleSpeed::Forward2x
        );
        assert_eq!(
            handle.play_backward().unwrap().speed,
            ShuttleSpeed::Reverse1x
        );

        let status = handle.pause_playback().unwrap();
        assert!(!status.playing);
        assert_eq!(handle.playback_status().unwrap(), status);

        let status = handle
            .seek(RationalTime::new(48, Rational::FPS_24))
            .unwrap();
        assert_eq!(status.position, RationalTime::new(48, Rational::FPS_24));
        engine.shutdown().unwrap();
    }

    #[test]
    fn the_playhead_is_published_as_an_engine_event() {
        let (project, _) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let handle = engine.handle();
        let playhead = handle.subscribe_playhead();
        let edits = handle.subscribe();

        handle
            .playback(PlaybackOp::SetTimebase {
                rate: Rational::FPS_24,
                duration: RationalTime::new(240, Rational::FPS_24),
            })
            .unwrap();
        let event = playhead.recv().unwrap();
        assert_eq!(event.position, RationalTime::new(0, Rational::FPS_24));
        assert!(!event.playing);

        handle.play_forward().unwrap();
        assert!(playhead.recv().unwrap().playing);

        // The clock runs on the engine thread: a few frames later the
        // playhead has moved on its own, and nothing about it was a project
        // change, so the editing stream stayed silent.
        let moved = playhead
            .recv_timeout(Duration::from_secs(2))
            .expect("the clock should publish a frame");
        assert!(moved.position.value() > 0, "{moved:?}");
        assert_eq!(moved.speed, ShuttleSpeed::Forward1x);
        assert_eq!(edits.try_recv(), None);

        handle.pause_playback().unwrap();
        engine.shutdown().unwrap();
    }

    #[test]
    fn playing_off_the_end_stops_and_looping_wraps() {
        let (project, _) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let handle = engine.handle();
        handle
            .playback(PlaybackOp::SetTimebase {
                rate: Rational::FPS_24,
                duration: RationalTime::new(2, Rational::FPS_24),
            })
            .unwrap();
        handle.play_forward().unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let stopped = loop {
            let status = handle.playback_status().unwrap();
            if !status.playing {
                break status;
            }
            assert!(Instant::now() < deadline, "playback never reached the end");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(stopped.position, RationalTime::new(1, Rational::FPS_24));

        let range = TimeRange::from_start_end(
            RationalTime::new(0, Rational::FPS_24),
            RationalTime::new(2, Rational::FPS_24),
        )
        .unwrap();
        let status = handle.set_loop_range(Some(range)).unwrap();
        assert_eq!(status.loop_range, Some(range));
        handle.seek(RationalTime::new(0, Rational::FPS_24)).unwrap();
        handle.play_forward().unwrap();
        thread::sleep(Duration::from_millis(300));
        // With a loop range in force it is still running several seconds of
        // frames later, rather than having stopped at the end.
        assert!(handle.playback_status().unwrap().playing);
        engine.shutdown().unwrap();
    }

    #[test]
    fn an_empty_loop_range_and_an_unknown_sequence_are_refused() {
        let (project, _) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let handle = engine.handle();
        let empty = TimeRange::empty_at(RationalTime::new(0, Rational::FPS_24));
        assert_eq!(
            handle.set_loop_range(Some(empty)).unwrap_err().code,
            codes::INVALID_TIME
        );
        assert_eq!(
            handle.follow_sequence(SequenceId::new()).unwrap_err().code,
            codes::SEQUENCE_NOT_FOUND
        );
        engine.shutdown().unwrap();
    }

    #[test]
    fn applying_publishes_a_snapshot_and_an_event() {
        let (project, sequence) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let events = engine.handle().subscribe();

        assert_eq!(engine.handle().revision(), 0);
        let applied = engine.handle().apply(add_track(sequence, "V1")).unwrap();

        assert_eq!(applied.revision, 1);
        assert_eq!(engine.handle().revision(), 1);
        assert_eq!(applied.project.sequences[0].tracks.len(), 1);
        assert_eq!(engine.handle().snapshot().sequences[0].tracks.len(), 1);

        let event = events.recv().unwrap();
        assert_eq!(event.entity, EntityKind::TRACK);
        assert_eq!(event.change, ChangeType::Added);
        assert_eq!(event.origin, ChangeOrigin::Apply);
        assert_eq!(applied.events, vec![event]);
    }

    #[test]
    fn an_older_snapshot_is_untouched_by_later_commands() {
        let (project, sequence) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let before = engine.handle().snapshot();
        engine.handle().apply(add_track(sequence, "V1")).unwrap();

        assert!(before.sequences[0].tracks.is_empty());
        assert_eq!(engine.handle().snapshot().sequences[0].tracks.len(), 1);
    }

    #[test]
    fn a_failing_command_changes_nothing() {
        let (project, sequence) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let events = engine.handle().subscribe();

        let err = engine
            .handle()
            .apply(RenameTrack {
                sequence,
                track: sub_model::TrackId::new(),
                name: "V9".to_owned(),
            })
            .unwrap_err();

        assert_eq!(err.code, codes::TRACK_NOT_FOUND);
        assert_eq!(engine.handle().revision(), 0);
        assert_eq!(events.try_recv(), None);
    }

    #[test]
    fn undo_and_redo_emit_inverted_and_replayed_events() {
        let (project, sequence) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let track = {
            engine.handle().apply(add_track(sequence, "V1")).unwrap();
            engine.handle().snapshot().sequences[0].tracks[0].id
        };
        let events = engine.handle().subscribe();

        engine
            .handle()
            .apply(RemoveTrack {
                sequence,
                track,
                force: false,
            })
            .unwrap();
        let removed = events.recv().unwrap();
        assert_eq!(removed.change, ChangeType::Removed);
        assert_eq!(removed.id.as_deref(), Some(track.to_string().as_str()));

        let undone = engine.handle().undo().unwrap().unwrap();
        assert_eq!(undone.revision, 3);
        let event = events.recv().unwrap();
        assert_eq!(event.origin, ChangeOrigin::Undo);
        assert_eq!(event.change, ChangeType::Added);
        assert_eq!(engine.handle().snapshot().sequences[0].tracks.len(), 1);

        let redone = engine.handle().redo().unwrap().unwrap();
        assert_eq!(redone.revision, 4);
        let event = events.recv().unwrap();
        assert_eq!(event.origin, ChangeOrigin::Redo);
        assert_eq!(event.change, ChangeType::Removed);
        assert!(engine.handle().snapshot().sequences[0].tracks.is_empty());
    }

    #[test]
    fn undo_on_an_empty_history_is_not_an_error() {
        let (project, _) = fixture();
        let engine = Engine::spawn(project).unwrap();
        assert!(engine.handle().undo().unwrap().is_none());
        assert!(engine.handle().redo().unwrap().is_none());
        assert_eq!(engine.handle().revision(), 0);
    }

    #[test]
    fn a_group_is_one_undo_step_and_an_abort_reverses_it() {
        let (project, sequence) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let events = engine.handle().subscribe();

        engine.handle().begin_group("Add tracks").unwrap();
        engine.handle().apply(add_track(sequence, "V1")).unwrap();
        engine.handle().apply(add_track(sequence, "V2")).unwrap();
        assert!(engine.handle().commit_group().unwrap());

        let summary = engine.handle().history().unwrap();
        assert_eq!(summary.undo_len, 1);
        assert_eq!(summary.undo_label.as_deref(), Some("Add tracks"));
        assert!(!summary.in_group);

        engine.handle().begin_group("Add more").unwrap();
        engine.handle().apply(add_track(sequence, "V3")).unwrap();
        engine.handle().abort_group().unwrap();
        assert_eq!(engine.handle().snapshot().sequences[0].tracks.len(), 2);
        assert_eq!(engine.handle().history().unwrap().undo_len, 1);

        let kinds: Vec<ChangeType> = std::iter::from_fn(|| events.try_recv())
            .map(|event| event.change)
            .collect();
        assert_eq!(
            kinds,
            vec![
                ChangeType::Added,
                ChangeType::Added,
                ChangeType::Added,
                ChangeType::Removed,
            ]
        );
    }

    #[test]
    fn envelopes_are_decoded_with_the_registry() {
        let (project, sequence) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let envelope = CommandEnvelope::new(
            "track.add",
            serde_json::json!({ "sequence": sequence.to_string(), "name": "V1", "kind": "video" }),
        );
        engine.handle().apply_envelope(envelope).unwrap();
        assert_eq!(engine.handle().snapshot().sequences[0].tracks.len(), 1);

        let err = engine
            .handle()
            .apply_envelope(CommandEnvelope::new("track.nope", serde_json::json!({})))
            .unwrap_err();
        assert_eq!(err.code, codes::UNKNOWN_COMMAND);
    }

    #[test]
    fn a_handle_to_a_stopped_engine_reports_it() {
        let (project, sequence) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let handle = engine.handle().clone();
        engine.shutdown().unwrap();

        let err = handle.apply(add_track(sequence, "V1")).unwrap_err();
        assert_eq!(err.code, codes::ENGINE_STOPPED);
    }

    #[test]
    fn shutting_down_closes_the_event_bus() {
        let (project, _) = fixture();
        let engine = Engine::spawn(project).unwrap();
        let events = engine.handle().subscribe();
        engine.shutdown().unwrap();
        assert_eq!(events.recv(), None);
        assert!(events.is_closed());
    }
}
