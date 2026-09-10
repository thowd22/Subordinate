//! The change event broadcast bus.
//!
//! One [`EventBus`] fans every [`ChangeEvent`] the engine produces out to every
//! subscriber: the UI, the Command API's subscriptions, the MCP bridge. The bus
//! is generic over what it carries and defaults to [`ChangeEvent`], so the
//! engine runs a second one for the playhead
//! ([`crate::playback::PlayheadEvent`]) without a second implementation. Two
//! properties matter more than throughput here:
//!
//! - **The engine never blocks on a subscriber.** Each subscriber has a bounded
//!   queue; a subscriber that stops draining loses its oldest events and is
//!   told how many, rather than stalling every editing command in the app
//!   (docs/PLAN.md §4).
//! - **A dropped subscriber unsubscribes itself.** The bus holds weak
//!   references, so a panel that goes away is pruned on the next publish.
//!
//! ```
//! use sub_edit::{ChangeEvent, ChangeOrigin, CommandEnvelope, EventBus};
//!
//! let bus = EventBus::new(8).unwrap();
//! let receiver = bus.subscribe();
//! let envelope = CommandEnvelope::new("track.remove", serde_json::json!({ "track": "t" }));
//! bus.publish(vec![ChangeEvent::from_envelope(1, &envelope, ChangeOrigin::Apply)]);
//!
//! let event = receiver.try_recv().unwrap();
//! assert_eq!(event.command, "track.remove");
//! assert_eq!(receiver.try_recv(), None);
//! ```

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use sub_core::{SubError, SubResult, codes as core_codes};

use crate::event::ChangeEvent;

/// The per-subscriber queue depth [`EventBus::default`] uses.
pub const DEFAULT_EVENT_CAPACITY: usize = 1024;

/// The queue behind one subscriber.
#[derive(Debug)]
struct Queue<E> {
    events: VecDeque<E>,
    lagged: u64,
    closed: bool,
}

/// The state one subscriber and the bus share.
#[derive(Debug)]
struct Shared<E> {
    queue: Mutex<Queue<E>>,
    ready: Condvar,
    capacity: usize,
}

impl<E> Shared<E> {
    /// Pushes one event, dropping the oldest and counting the loss when the
    /// subscriber is not keeping up. Never blocks.
    fn push(&self, event: E) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.closed {
            return;
        }
        if queue.events.len() == self.capacity {
            queue.events.pop_front();
            queue.lagged = queue.lagged.saturating_add(1);
        }
        queue.events.push_back(event);
        drop(queue);
        self.ready.notify_all();
    }

    /// Marks the queue closed so blocked receivers wake and stop waiting.
    fn close(&self) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.closed = true;
        drop(queue);
        self.ready.notify_all();
    }
}

/// A broadcast channel from the engine to any number of subscribers.
#[derive(Debug)]
pub struct EventBus<E = ChangeEvent> {
    subscribers: Mutex<Vec<Weak<Shared<E>>>>,
    capacity: usize,
}

impl<E> Default for EventBus<E> {
    fn default() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            capacity: DEFAULT_EVENT_CAPACITY,
        }
    }
}

impl<E: Clone> EventBus<E> {
    /// A bus giving every subscriber a queue `capacity` events deep.
    ///
    /// # Errors
    ///
    /// Returns `core.invalid_argument` when `capacity` is zero: a subscriber
    /// that can hold nothing would drop every event it is sent.
    pub fn new(capacity: usize) -> SubResult<Self> {
        if capacity == 0 {
            return Err(SubError::new(
                core_codes::INVALID_ARGUMENT,
                "event queue capacity must be at least one",
            ));
        }
        Ok(Self {
            subscribers: Mutex::new(Vec::new()),
            capacity,
        })
    }

    /// Adds a subscriber. Events published before this call are not delivered.
    #[must_use]
    pub fn subscribe(&self) -> EventReceiver<E> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                events: VecDeque::new(),
                lagged: 0,
                closed: false,
            }),
            ready: Condvar::new(),
            capacity: self.capacity,
        });
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        subscribers.retain(|weak| weak.strong_count() > 0);
        subscribers.push(Arc::downgrade(&shared));
        drop(subscribers);
        EventReceiver { shared }
    }

    /// Delivers `events` to every live subscriber, in order, without blocking.
    pub fn publish(&self, events: impl IntoIterator<Item = E>) {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        subscribers.retain(|weak| weak.strong_count() > 0);
        if subscribers.is_empty() {
            return;
        }
        for event in events {
            for weak in subscribers.iter() {
                if let Some(shared) = weak.upgrade() {
                    shared.push(event.clone());
                }
            }
        }
    }

    /// Closes every subscriber: queued events are still delivered, and
    /// [`EventReceiver::recv`] returns `None` once they run out.
    pub fn close(&self) {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for weak in subscribers.drain(..) {
            if let Some(shared) = weak.upgrade() {
                shared.close();
            }
        }
    }

    /// The number of subscribers still listening.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        subscribers.retain(|weak| weak.strong_count() > 0);
        subscribers.len()
    }
}

/// One subscriber's end of an [`EventBus`].
///
/// Dropping it unsubscribes. It is `Send` but not `Clone`: each listener holds
/// its own queue so a slow one cannot make another lose events.
#[derive(Debug)]
pub struct EventReceiver<E = ChangeEvent> {
    shared: Arc<Shared<E>>,
}

impl<E> EventReceiver<E> {
    /// Takes the next event if one is already queued.
    #[must_use]
    pub fn try_recv(&self) -> Option<E> {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        queue.events.pop_front()
    }

    /// Waits for the next event, returning `None` once the bus is closed and
    /// the queue is empty.
    #[must_use]
    pub fn recv(&self) -> Option<E> {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(event) = queue.events.pop_front() {
                return Some(event);
            }
            if queue.closed {
                return None;
            }
            queue = self
                .shared
                .ready
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Waits up to `timeout` for the next event.
    ///
    /// Returns `None` on timeout as well as on close, so a caller that must
    /// tell them apart checks [`EventReceiver::is_closed`].
    #[must_use]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<E> {
        let deadline = Instant::now() + timeout;
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(event) = queue.events.pop_front() {
                return Some(event);
            }
            if queue.closed {
                return None;
            }
            let remaining = deadline.checked_duration_since(Instant::now())?;
            let (guard, _) = self
                .shared
                .ready
                .wait_timeout(queue, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            queue = guard;
        }
    }

    /// The number of events dropped because this subscriber fell behind, and
    /// resets the count. A non-zero value means the listener missed changes and
    /// should re-read the snapshot rather than trust its incremental state.
    pub fn take_lagged(&self) -> u64 {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut queue.lagged)
    }

    /// The number of events waiting to be taken.
    #[must_use]
    pub fn len(&self) -> usize {
        let queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        queue.events.len()
    }

    /// Whether no event is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the bus has been closed, which happens when the engine stops.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        let queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        queue.closed
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use serde_json::json;

    use super::*;
    use crate::command::CommandEnvelope;
    use crate::event::ChangeOrigin;

    fn event(revision: u64) -> ChangeEvent {
        ChangeEvent::from_envelope(
            revision,
            &CommandEnvelope::new("track.rename", json!({ "track": "t" })),
            ChangeOrigin::Apply,
        )
    }

    #[test]
    fn capacity_must_not_be_zero() {
        let err = EventBus::<ChangeEvent>::new(0).unwrap_err();
        assert_eq!(err.code, core_codes::INVALID_ARGUMENT);
    }

    #[test]
    fn every_subscriber_gets_every_event_in_order() {
        let bus = EventBus::new(4).unwrap();
        let first = bus.subscribe();
        let second = bus.subscribe();
        bus.publish([event(1), event(2)]);

        for receiver in [&first, &second] {
            assert_eq!(receiver.recv().unwrap().revision, 1);
            assert_eq!(receiver.recv().unwrap().revision, 2);
            assert!(receiver.is_empty());
            assert_eq!(receiver.take_lagged(), 0);
        }
    }

    #[test]
    fn a_slow_subscriber_loses_the_oldest_events_and_is_told() {
        let bus = EventBus::new(2).unwrap();
        let receiver = bus.subscribe();
        bus.publish((1..=5).map(event));

        assert_eq!(receiver.len(), 2);
        assert_eq!(receiver.recv().unwrap().revision, 4);
        assert_eq!(receiver.recv().unwrap().revision, 5);
        assert_eq!(receiver.take_lagged(), 3);
        assert_eq!(receiver.take_lagged(), 0);
    }

    #[test]
    fn a_dropped_subscriber_is_pruned() {
        let bus = EventBus::new(2).unwrap();
        let receiver = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);
        drop(receiver);
        bus.publish([event(1)]);
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn recv_blocks_until_an_event_arrives_and_stops_on_close() {
        let bus = Arc::new(EventBus::new(4).unwrap());
        let receiver = bus.subscribe();
        let publisher = Arc::clone(&bus);
        let handle = thread::spawn(move || {
            publisher.publish([event(1)]);
            publisher.close();
        });

        assert_eq!(receiver.recv().unwrap().revision, 1);
        assert_eq!(receiver.recv(), None);
        assert!(receiver.is_closed());
        handle.join().unwrap();
    }

    #[test]
    fn recv_timeout_gives_up() {
        let bus = EventBus::<ChangeEvent>::new(4).unwrap();
        let receiver = bus.subscribe();
        assert_eq!(receiver.recv_timeout(Duration::from_millis(10)), None);
        assert!(!receiver.is_closed());
    }
}
