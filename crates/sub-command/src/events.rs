//! Event subscription: change notifications pushed to a connected client.
//!
//! An MCP client or a pop-out window must learn that the project changed
//! without polling `project.revision` (docs/PLAN.md §4). A client calls
//! [`EVENTS_SUBSCRIBE`], gets a subscription id back, and from then on the
//! server sends it an [`EVENTS_CHANGED`] JSON-RPC notification for every
//! [`ChangeEvent`] the engine publishes, until it calls [`EVENTS_UNSUBSCRIBE`]
//! or disconnects.
//!
//! Subscriptions belong to a connection, not to the process, so the state
//! lives in a [`Session`] the transport creates per client. Everything a
//! session sends leaves through its [`Outbox`], including ordinary responses,
//! so one writer owns the socket and the newline framing cannot interleave.
//!
//! **A slow client never stalls the engine.** The engine's own bus already
//! drops the oldest events for a subscriber that stops draining; here the
//! outbox is bounded as well, and a client that lets it fill is dropped with a
//! logged warning rather than given back-pressure that would reach the engine
//! thread.
//!
//! ```
//! use std::sync::Arc;
//!
//! use sub_command::Dispatcher;
//! use sub_command::events::Session;
//! use sub_edit::Engine;
//! use sub_model::Project;
//!
//! let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
//! let dispatcher = Dispatcher::new(engine.handle().clone());
//! let session = Arc::new(Session::new(8));
//!
//! let answer = dispatcher
//!     .handle_text_in(
//!         Some(&session),
//!         r#"{"jsonrpc":"2.0","method":"events.subscribe","id":1}"#,
//!     )
//!     .unwrap();
//! assert!(answer.contains("subscription"));
//!
//! dispatcher
//!     .invoke("bin.create", Some(serde_json::json!({ "name": "Footage" })))
//!     .unwrap();
//! let notification = session.outbox().take().unwrap();
//! assert!(notification.contains("events.changed"));
//!
//! session.close();
//! engine.shutdown().unwrap();
//! ```

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_edit::{ChangeEvent, EngineHandle};
use tracing::{debug, warn};

use crate::codes;
use crate::rpc::Notification;

/// Subscribe to change events on this connection.
pub const EVENTS_SUBSCRIBE: &str = "events.subscribe";
/// Stop a subscription made on this connection.
pub const EVENTS_UNSUBSCRIBE: &str = "events.unsubscribe";
/// The notification method a subscription delivers.
pub const EVENTS_CHANGED: &str = "events.changed";

/// How many messages a connection may have waiting before it is dropped.
///
/// A client reading its socket at any reasonable rate never approaches this;
/// one that has stopped reading altogether is dead weight the editor sheds.
pub const DEFAULT_OUTBOX_CAPACITY: usize = 4096;

/// How often a subscription's pump wakes to notice it has been cancelled.
///
/// The engine's receiver has no interruptible wait, so the pump waits in short
/// steps instead; the cost is one wake-up per interval on an idle connection.
const PUMP_POLL: Duration = Duration::from_millis(20);

/// The identifier [`EVENTS_SUBSCRIBE`] hands back.
///
/// It is unique within one connection, which is all a client needs: it can
/// only unsubscribe its own subscriptions.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SubscriptionId(String);

impl SubscriptionId {
    /// The identifier as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The result of [`EVENTS_SUBSCRIBE`].
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct SubscribeResult {
    /// The identifier to pass to [`EVENTS_UNSUBSCRIBE`].
    pub subscription: SubscriptionId,
}

/// The result of [`EVENTS_UNSUBSCRIBE`].
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct UnsubscribeResult {
    /// The subscription that was stopped.
    pub subscription: SubscriptionId,
}

/// The parameters of [`EVENTS_UNSUBSCRIBE`].
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnsubscribeParams {
    /// The subscription to stop.
    pub subscription: SubscriptionId,
}

/// The parameters of an [`EVENTS_CHANGED`] notification.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ChangedParams {
    /// The subscription this event belongs to.
    pub subscription: SubscriptionId,
    /// What changed.
    pub event: ChangeEvent,
    /// How many events were dropped before this one because the subscriber
    /// fell behind. Non-zero means the client's incremental picture is stale
    /// and it should read `project.get` again.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lagged: u64,
}

/// Whether a lag count is worth putting on the wire.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if hands the field over by reference"
)]
fn is_zero(lagged: &u64) -> bool {
    *lagged == 0
}

/// What one connection is waiting to send.
#[derive(Debug)]
struct OutboxState {
    messages: VecDeque<String>,
    closed: bool,
    overflowed: bool,
}

/// The single queue every message a connection sends passes through.
///
/// [`Outbox::push`] never blocks and never waits on the reader: a full outbox
/// closes itself, which is how a slow client is dropped instead of being
/// allowed to hold up whoever is writing to it.
#[derive(Debug)]
pub struct Outbox {
    state: Mutex<OutboxState>,
    ready: Condvar,
    capacity: usize,
}

impl Outbox {
    /// An outbox holding at most `capacity` messages.
    ///
    /// A capacity of zero is raised to one so that one message can always be
    /// in flight.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(OutboxState {
                messages: VecDeque::new(),
                closed: false,
                overflowed: false,
            }),
            ready: Condvar::new(),
            capacity: capacity.max(1),
        }
    }

    /// Queues one message, returning whether it was accepted.
    ///
    /// A message is refused when the outbox is closed, and the outbox closes
    /// itself when it is already full: the client is not keeping up and the
    /// connection is forfeit.
    pub fn push(&self, message: String) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return false;
        }
        if state.messages.len() >= self.capacity {
            state.closed = true;
            state.overflowed = true;
            drop(state);
            self.ready.notify_all();
            warn!(
                capacity = self.capacity,
                "dropping a client that stopped reading its Command API connection",
            );
            return false;
        }
        state.messages.push_back(message);
        drop(state);
        self.ready.notify_all();
        true
    }

    /// Waits for the next message, returning `None` once the outbox is closed
    /// and drained.
    #[must_use]
    pub fn take(&self) -> Option<String> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(message) = state.messages.pop_front() {
                return Some(message);
            }
            if state.closed {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Closes the outbox: queued messages are still taken, and [`Outbox::take`]
    /// returns `None` after them.
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.closed = true;
        drop(state);
        self.ready.notify_all();
    }

    /// Whether the outbox has been closed, by [`Outbox::close`] or by
    /// overflowing.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed
    }

    /// Whether the outbox closed because the client stopped reading.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .overflowed
    }

    /// How many messages are waiting.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .messages
            .len()
    }

    /// Whether nothing is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One live subscription: the thread pumping the engine's events into the
/// session's outbox, and the flag that stops it.
#[derive(Debug)]
struct Subscription {
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
}

impl Subscription {
    /// Stops the pump and waits for it, so an unsubscribed client is
    /// guaranteed no further notification.
    fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(pump) = self.pump.take()
            && pump.join().is_err()
        {
            warn!("an event pump thread panicked");
        }
    }
}

/// Everything one client connection owns.
///
/// The transport makes one per accepted connection and drops it when the
/// client goes away, which cancels every subscription that connection made.
#[derive(Debug)]
pub struct Session {
    outbox: Arc<Outbox>,
    subscriptions: Mutex<BTreeMap<SubscriptionId, Subscription>>,
    next_id: AtomicU64,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(DEFAULT_OUTBOX_CAPACITY)
    }
}

impl Session {
    /// A session whose outbox holds at most `capacity` messages.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            outbox: Arc::new(Outbox::new(capacity)),
            subscriptions: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// The queue everything this connection sends passes through.
    #[must_use]
    pub fn outbox(&self) -> &Arc<Outbox> {
        &self.outbox
    }

    /// Queues one already-encoded message for the client.
    ///
    /// Returns whether it was accepted; `false` means the connection is over.
    pub fn send(&self, message: String) -> bool {
        self.outbox.push(message)
    }

    /// How many subscriptions this connection holds.
    #[must_use]
    pub fn subscription_count(&self) -> usize {
        self.subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Starts a subscription on `engine` and returns its identifier.
    ///
    /// # Errors
    ///
    /// Returns `command.transport_io` when the pump thread cannot be started.
    pub fn subscribe(&self, engine: &EngineHandle) -> SubResult<SubscriptionId> {
        let id = SubscriptionId(format!(
            "sub-{}",
            self.next_id.fetch_add(1, Ordering::SeqCst)
        ));
        let receiver = engine.subscribe();
        let stop = Arc::new(AtomicBool::new(false));
        let pump = {
            let stop = Arc::clone(&stop);
            let outbox = Arc::clone(&self.outbox);
            let id = id.clone();
            thread::Builder::new()
                .name("sub-command-events".to_owned())
                .spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        let Some(event) = receiver.recv_timeout(PUMP_POLL) else {
                            if receiver.is_closed() {
                                break;
                            }
                            continue;
                        };
                        let lagged = receiver.take_lagged();
                        if !outbox.push(changed_message(&id, event, lagged)) {
                            break;
                        }
                    }
                    debug!(subscription = %id, "an event subscription has ended");
                })
                .map_err(|error| {
                    SubError::new(
                        codes::TRANSPORT_IO,
                        "the event subscription thread could not be started",
                    )
                    .with_cause(&error)
                })?
        };
        self.subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id.clone(),
                Subscription {
                    stop,
                    pump: Some(pump),
                },
            );
        Ok(id)
    }

    /// Stops one subscription.
    ///
    /// # Errors
    ///
    /// Returns `command.unknown_subscription` when this connection has no such
    /// subscription, which includes one it has already stopped.
    pub fn unsubscribe(&self, id: &SubscriptionId) -> SubResult<()> {
        let subscription = self
            .subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        match subscription {
            Some(subscription) => {
                subscription.stop();
                Ok(())
            }
            None => Err(SubError::new(
                codes::UNKNOWN_SUBSCRIPTION,
                "this connection has no such subscription",
            )
            .with_detail("subscription", id.to_string())),
        }
    }

    /// Stops every subscription and closes the outbox.
    ///
    /// Called when the client disconnects; calling it twice is harmless.
    pub fn close(&self) {
        let subscriptions = std::mem::take(
            &mut *self
                .subscriptions
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for (_, subscription) in subscriptions {
            subscription.stop();
        }
        self.outbox.close();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

/// Encodes one change event as the line a subscriber receives.
fn changed_message(id: &SubscriptionId, event: ChangeEvent, lagged: u64) -> String {
    let params = ChangedParams {
        subscription: id.clone(),
        event,
        lagged,
    };
    let notification = Notification::new(
        EVENTS_CHANGED,
        Some(serde_json::to_value(&params).unwrap_or_default()),
    );
    serde_json::to_string(&notification).expect("a notification is always serialisable")
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;
    use sub_edit::{CommandEnvelope, Engine};
    use sub_model::Project;

    use super::{Outbox, Session, SubscriptionId};
    use crate::codes;

    /// Applies a bin creation, the shortest real command there is.
    fn create_bin(engine: &Engine, name: &str) {
        engine
            .handle()
            .apply_envelope(CommandEnvelope::new("bin.create", json!({ "name": name })))
            .unwrap();
    }

    /// Waits for the next message, failing rather than hanging the suite.
    fn take_soon(outbox: &Outbox) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if !outbox.is_empty() {
                return outbox.take().expect("a message is waiting");
            }
            assert!(Instant::now() < deadline, "no message arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_subscription_delivers_every_change() {
        let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
        let session = Session::new(16);
        let id = session.subscribe(engine.handle()).unwrap();
        assert_eq!(session.subscription_count(), 1);

        create_bin(&engine, "Footage");

        let message = take_soon(session.outbox());
        assert!(message.contains("\"events.changed\""), "{message}");
        assert!(message.contains(id.as_str()), "{message}");

        session.close();
        engine.shutdown().unwrap();
    }

    #[test]
    fn unsubscribing_stops_delivery_and_is_reported_once() {
        let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
        let session = Session::new(16);
        let id = session.subscribe(engine.handle()).unwrap();
        session.unsubscribe(&id).unwrap();
        assert_eq!(session.subscription_count(), 0);
        assert_eq!(engine.handle().subscriber_count(), 0);

        create_bin(&engine, "Footage");
        assert!(session.outbox().is_empty());

        let error = session.unsubscribe(&id).unwrap_err();
        assert_eq!(error.code, codes::UNKNOWN_SUBSCRIPTION);

        session.close();
        engine.shutdown().unwrap();
    }

    #[test]
    fn a_full_outbox_closes_itself_rather_than_blocking() {
        let outbox = Outbox::new(2);
        assert!(outbox.push("one".to_owned()));
        assert!(outbox.push("two".to_owned()));
        assert!(!outbox.push("three".to_owned()));
        assert!(outbox.overflowed());
        assert!(outbox.is_closed());

        // What was already queued is still delivered, then the reader stops.
        assert_eq!(outbox.take().as_deref(), Some("one"));
        assert_eq!(outbox.take().as_deref(), Some("two"));
        assert_eq!(outbox.take(), None);
    }

    #[test]
    fn a_slow_subscriber_is_dropped_and_the_engine_carries_on() {
        let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
        let session = Session::new(1);
        session.subscribe(engine.handle()).unwrap();

        // Far more edits than the outbox can hold, none of them read.
        for index in 0..64 {
            create_bin(&engine, &format!("Bin {index}"));
        }
        assert_eq!(engine.handle().revision(), 64);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !session.outbox().overflowed() {
            assert!(Instant::now() < deadline, "the outbox never overflowed");
            std::thread::sleep(Duration::from_millis(5));
        }

        session.close();
        engine.shutdown().unwrap();
    }

    #[test]
    fn closing_a_session_ends_its_subscriptions() {
        let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
        let session = Session::new(8);
        session.subscribe(engine.handle()).unwrap();
        session.subscribe(engine.handle()).unwrap();
        assert_eq!(session.subscription_count(), 2);

        session.close();
        assert_eq!(session.subscription_count(), 0);
        assert!(session.outbox().is_closed());
        assert!(!session.send("late".to_owned()));
        assert_eq!(engine.handle().subscriber_count(), 0);

        engine.shutdown().unwrap();
    }

    #[test]
    fn identifiers_are_distinct_within_a_session() {
        let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
        let session = Session::new(8);
        let first = session.subscribe(engine.handle()).unwrap();
        let second = session.subscribe(engine.handle()).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.as_str(), "sub-1");
        assert_eq!(second, SubscriptionId("sub-2".to_owned()));

        session.close();
        engine.shutdown().unwrap();
    }
}
