//! Resource updates, carried from the editor's event bus to MCP subscribers.
//!
//! A resource is only worth caching if the client is told when it goes stale.
//! The editor already broadcasts one [`sub_edit::ChangeEvent`] per applied
//! command, and the Command API already delivers those to a connected client
//! as `events.changed` notifications (`sub_command::events`). This module is
//! the join: one Command API connection of its own, subscribed to the bus, a
//! thread reading it, and a broadcast channel that fans each
//! [`Update`](crate::resources::Update) out to every MCP subscription the
//! bridge is serving.
//!
//! The feed is opened on the first subscription and lives as long as the
//! process: an MCP session is one bridge and one editor connection, and a feed
//! that is torn down and rebuilt per subscription would miss the events in
//! between. It is a *second* connection because the first one is a strict
//! request-and-reply channel shared by every tool call, and notifications must
//! not arrive in the middle of a round trip on it.
//!
//! A subscriber that stops reading lags rather than blocking: the channel is
//! bounded, the oldest updates are dropped, and the lag is reported so the
//! subscriber can be told to read everything again. Nothing here can slow the
//! engine down.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use sub_command::events::{EVENTS_CHANGED, EVENTS_SUBSCRIBE};
use sub_command::transport::Client;
use sub_core::{SubError, SubResult};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::backend::Backend;
use crate::codes;
use crate::resources::Update;

/// How many updates a subscriber may fall behind before it starts losing them.
///
/// A subscriber that lags is told so and reads the resources it cares about
/// again, so the only cost of the bound is that read.
pub const FEED_CAPACITY: usize = 256;

/// The change feed the bridge's subscriptions read.
#[derive(Debug)]
pub struct Watch {
    /// Where the second connection is made.
    backend: Arc<Backend>,
    /// The running feed, once something has asked for one.
    feed: Mutex<Option<Feed>>,
}

/// One subscription to the editor's event bus, fanned out to subscribers.
#[derive(Debug)]
struct Feed {
    /// The end the pump publishes on.
    sender: broadcast::Sender<Update>,
    /// Whether the pump is still reading; it stops when the editor goes away.
    alive: Arc<AtomicBool>,
}

impl Watch {
    /// A watch that will read `backend`'s editor when it is first asked to.
    #[must_use]
    pub fn new(backend: Arc<Backend>) -> Self {
        Self {
            backend,
            feed: Mutex::new(None),
        }
    }

    /// A receiver of every resource update from now on, starting the feed if
    /// it is not running.
    ///
    /// This blocks: it opens a connection and makes one Command API call, so
    /// an asynchronous caller belongs on a blocking task.
    ///
    /// # Errors
    ///
    /// Returns `mcp.watch_failed` when the editor cannot be watched, and
    /// whatever `events.subscribe` itself returned.
    pub fn updates(&self) -> SubResult<broadcast::Receiver<Update>> {
        let mut feed = self.feed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(running) = feed.as_ref()
            && running.alive.load(Ordering::Acquire)
        {
            return Ok(running.sender.subscribe());
        }
        let started = self.start()?;
        let receiver = started.sender.subscribe();
        *feed = Some(started);
        Ok(receiver)
    }

    /// Whether a feed is running.
    #[must_use]
    pub fn is_watching(&self) -> bool {
        self.feed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|feed| feed.alive.load(Ordering::Acquire))
    }

    /// Opens the connection, subscribes to the bus and starts the pump.
    fn start(&self) -> SubResult<Feed> {
        let mut client = self.backend.open_events()?;
        let subscription = client.invoke(EVENTS_SUBSCRIBE, None)?;
        debug!(
            subscription = %subscription["subscription"],
            "watching the editor's event bus for resource updates",
        );
        let (sender, _) = broadcast::channel(FEED_CAPACITY);
        let alive = Arc::new(AtomicBool::new(true));
        let publisher = sender.clone();
        let running = Arc::clone(&alive);
        std::thread::Builder::new()
            .name("mcp-resource-watch".to_owned())
            .spawn(move || {
                pump(client, &publisher);
                running.store(false, Ordering::Release);
            })
            .map_err(|error| {
                SubError::new(
                    codes::WATCH_FAILED,
                    "the resource watch thread could not be started",
                )
                .with_cause(&error)
            })?;
        Ok(Feed { sender, alive })
    }
}

/// Reads change events until the connection ends, publishing what they stale.
fn pump(mut client: Client, sender: &broadcast::Sender<Update>) {
    loop {
        match client.recv_notification() {
            Ok(Some(notification)) => {
                if notification.method != EVENTS_CHANGED {
                    continue;
                }
                let Some(params) = notification.params else {
                    continue;
                };
                // A dropped update means nobody is subscribed at this instant,
                // which is not a failure: the next subscriber reads the
                // resources it cares about when it subscribes.
                let _ = sender.send(Update::from_event(&params["event"]));
            }
            Ok(None) => {
                debug!("the editor closed the resource watch connection");
                break;
            }
            Err(error) => {
                warn!(
                    code = error.code.as_str(),
                    "the resource watch ended: {error}"
                );
                break;
            }
        }
    }
}
