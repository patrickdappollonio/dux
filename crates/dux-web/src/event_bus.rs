//! The web-layer event bus: a tiny `tokio::sync::broadcast` fan-out plus a
//! global per-topic interest refcount.
//!
//! Named `event_bus.rs` to avoid colliding with `dux-core`'s
//! `engine/events.rs`. It is a pure web concern held in [`crate::server::AppState`]
//! as an `Arc<EventBus>` beside the engine handle — no dux-core / engine-actor
//! touch.
//!
//! ## What it carries
//!
//! The bus carries resource-change *signals* only ([`Event::Resource`]): an event
//! names *what changed* (and a monotonic `rev` where the client needs ordering),
//! never the changed value. The client decides whether to issue a REST GET in
//! response. Status/toast events are delivered on the SAME `/ws/events` socket
//! (scope-filtered), but ride the engine's status broadcast rather than this bus.
//!
//! There is deliberately NO bus variant for lag recovery: a lagged `/ws/events`
//! connection synthesizes its own catch-up frames directly to its sink (see the
//! `RecvError::Lagged` arm in `server.rs`). Putting a "resync" on the broadcast
//! bus would fan one slow connection's recovery out to every connection and could
//! itself fill the buffer.
//!
//! ## Interest
//!
//! Each `/ws/events` connection registers interest in the fine topics it is
//! subscribed to (e.g. `session:<id>:changes`). The interest map is a global
//! refcount so the changed-files poller does background git work ONLY for sessions
//! some client is actually showing (see [`EventBus::interested_sessions`]).

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::broadcast;

/// Capacity of the broadcast channel. Large enough that a briefly-slow connection
/// rarely lags; a connection that does lag arms `RecvError::Lagged` with
/// log-and-continue and synthesizes its own catch-up (see `server.rs`).
pub const EVENT_BUS_CAPACITY: usize = 1024;

/// A server -> client change signal. Phase 1 carries resource-change events only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A named resource changed. `id` scopes it to one resource instance and
    /// `rev` carries the monotonic revision where the client needs ordering/dedup
    /// (e.g. `session.changes`).
    Resource {
        event: String,
        id: Option<String>,
        rev: Option<u64>,
        /// The claiming connection's id for a `pty.owner` handover (the
        /// `PtySizeOwners` conn id, stringified). `None` for every other event.
        /// A client viewing that PTY compares it against its own PTY-socket
        /// connection id to decide definitively whether the handover is its own
        /// claim (stay owner) or a foreign takeover (show the read-only
        /// placeholder), replacing the old timing heuristic.
        owner: Option<String>,
        /// The monotonic ownership epoch for a `pty.owner` handover, assigned
        /// UNDER the [`PtySizeOwners`](crate::server) owners lock at the instant a
        /// new owner is recorded. Because it is bumped in the same critical
        /// section that serializes owner writes, epochs reflect TRUE claim order
        /// even when two connections claim at once. The `pty.owner` broadcast is
        /// emitted after the lock releases and can be reordered by the runtime, so
        /// clients keep only the highest epoch seen per pty and ignore any older
        /// arrival, converging on the latest claim. `None` for every other event.
        epoch: Option<u64>,
    },
}

/// The fine topic a session's changed-files signal is delivered on.
pub fn changes_topic(session_id: &str) -> String {
    format!("session:{session_id}:changes")
}

/// Extract the session id from a `session:<id>:changes` topic, or `None` if the
/// topic is not a session-changes topic. `<id>` may itself contain colons, so we
/// strip the fixed prefix/suffix rather than splitting on `:`.
pub fn session_id_from_changes_topic(topic: &str) -> Option<&str> {
    topic
        .strip_prefix("session:")
        .and_then(|rest| rest.strip_suffix(":changes"))
        .filter(|id| !id.is_empty())
}

/// The broadcast fan-out plus the global per-topic interest refcount.
pub struct EventBus {
    tx: broadcast::Sender<Event>,
    interest: Mutex<HashMap<String, usize>>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(EVENT_BUS_CAPACITY);
        Self {
            tx,
            interest: Mutex::new(HashMap::new()),
        }
    }

    /// A fresh receiver for one connection's event-forwarder loop.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Publish an event to every connected subscriber. A send error means there
    /// are no live receivers, which is normal (nobody connected) — ignore it.
    pub fn emit(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }

    /// Increment the interest refcount for `topic` (a connection just subscribed
    /// to a topic it was not already holding).
    pub fn add_interest(&self, topic: &str) {
        let mut map = self.interest.lock().unwrap();
        *map.entry(topic.to_string()).or_insert(0) += 1;
    }

    /// Decrement the interest refcount for `topic`, removing it at zero. Saturating
    /// and underflow-logged: dropping interest in a topic that was never counted is
    /// a bookkeeping bug, so it is logged rather than panicking or wrapping.
    pub fn drop_interest(&self, topic: &str) {
        let mut map = self.interest.lock().unwrap();
        match map.get_mut(topic) {
            Some(count) => {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    map.remove(topic);
                }
            }
            None => {
                dux_core::logger::warn(&format!(
                    "EventBus::drop_interest underflow for topic {topic:?} (no live interest)"
                ));
            }
        }
    }

    /// The session ids that currently have at least one `:changes` subscriber.
    /// Drives the changed-files poller: idle sessions (no subscriber) are never
    /// polled.
    pub fn interested_sessions(&self) -> Vec<String> {
        let map = self.interest.lock().unwrap();
        map.keys()
            .filter_map(|topic| session_id_from_changes_topic(topic))
            .map(|id| id.to_string())
            .collect()
    }

    /// Whether any connection currently holds interest in `topic`. Used by the
    /// poller's grace-eviction to decide a session's cache can be dropped.
    pub fn has_interest(&self, topic: &str) -> bool {
        self.interest.lock().unwrap().contains_key(topic)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_topic_round_trips() {
        let t = changes_topic("s1");
        assert_eq!(t, "session:s1:changes");
        assert_eq!(session_id_from_changes_topic(&t), Some("s1"));
    }

    #[test]
    fn session_id_parser_rejects_non_changes_topics() {
        assert_eq!(session_id_from_changes_topic("sessions"), None);
        assert_eq!(session_id_from_changes_topic("projects"), None);
        assert_eq!(session_id_from_changes_topic("session::changes"), None);
        assert_eq!(
            session_id_from_changes_topic("session:s1:working"),
            None,
            "a working topic is not a changes topic"
        );
        // An id containing a colon is preserved (we strip prefix/suffix, not split).
        assert_eq!(
            session_id_from_changes_topic("session:a:b:changes"),
            Some("a:b")
        );
    }

    #[test]
    fn interest_refcount_is_exact_under_duplicates() {
        let bus = EventBus::new();
        let topic = changes_topic("s1");
        bus.add_interest(&topic);
        bus.add_interest(&topic);
        assert_eq!(bus.interested_sessions(), vec!["s1".to_string()]);
        // First drop leaves one holder; the session is still interested.
        bus.drop_interest(&topic);
        assert_eq!(bus.interested_sessions(), vec!["s1".to_string()]);
        // Second drop returns the refcount to zero; the topic is removed entirely.
        bus.drop_interest(&topic);
        assert!(bus.interested_sessions().is_empty());
        assert!(!bus.has_interest(&topic));
    }

    #[test]
    fn drop_interest_underflow_is_saturating() {
        let bus = EventBus::new();
        // Dropping with no prior interest must not panic and must stay at zero.
        bus.drop_interest(&changes_topic("ghost"));
        assert!(bus.interested_sessions().is_empty());
    }

    #[tokio::test]
    async fn emit_reaches_subscribers() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.emit(Event::Resource {
            event: "session.changes".to_string(),
            id: Some("s1".to_string()),
            rev: Some(7),
            owner: None,
            epoch: None,
        });
        let ev = rx.recv().await.unwrap();
        assert_eq!(
            ev,
            Event::Resource {
                event: "session.changes".to_string(),
                id: Some("s1".to_string()),
                rev: Some(7),
                owner: None,
                epoch: None,
            }
        );
    }
}
