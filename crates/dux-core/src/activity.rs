//! A bounded, thread-safe tail of the web server's lifecycle events plus a live
//! active-connection count. The web `Console` (the producer, on many tokio
//! worker threads) pushes here; the in-TUI server status screen (the consumer,
//! on the engine-loop thread) reads a [`ActivityRing::snapshot`] each redraw.
//!
//! The buffer is intentionally lossy: it keeps only the most recent
//! [`ACTIVITY_CAP`] events and drops the oldest. There is deliberately no
//! scrollback — the status screen shows a fixed tail.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The maximum number of events the ring retains. Older events are dropped.
pub const ACTIVITY_CAP: usize = 50;

/// The tone of a captured activity event. The public mirror of the web
/// console's private `Tone`, so the TUI can re-color events with its theme
/// instead of parsing ANSI back out of a formatted line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityTone {
    Info,
    Ok,
    Warn,
    Error,
}

/// One captured lifecycle event. `hms` is the wall-clock `HH:MM:SS` formatted by
/// the producer, so the ring carries no clock dependency of its own.
#[derive(Clone, Debug)]
pub struct ActivityEvent {
    pub hms: String,
    pub tone: ActivityTone,
    pub message: String,
}

/// A point-in-time read of the ring: the event generation (for cheap
/// "did anything change?" checks), the live connection count, and the tail of
/// recent events (bounded by the caller's `max_events`).
#[derive(Clone, Debug)]
pub struct ActivitySnapshot {
    pub generation: u64,
    pub connections: usize,
    pub events: Vec<ActivityEvent>,
}

struct ActivityInner {
    events: Mutex<VecDeque<ActivityEvent>>,
    connections: AtomicUsize,
    /// Bumped on every push (including pushes that drop an older event), so a
    /// reader can detect new activity without copying the buffer.
    generation: AtomicU64,
}

/// A cheap-to-clone (`Arc`) shared handle to the activity buffer.
#[derive(Clone)]
pub struct ActivityRing(Arc<ActivityInner>);

impl Default for ActivityRing {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityRing {
    pub fn new() -> Self {
        Self(Arc::new(ActivityInner {
            events: Mutex::new(VecDeque::with_capacity(ACTIVITY_CAP)),
            connections: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
        }))
    }

    /// Append an event, dropping the oldest if the buffer is at capacity, then
    /// bump the generation — all while holding the lock so a concurrent
    /// [`Self::snapshot`] can never observe the new event paired with the old
    /// generation (which would make the reader miss a redraw for that event).
    ///
    /// A poisoned lock is recovered rather than propagated: this is a lossy,
    /// display-only buffer, so a prior panic must not turn every later push into
    /// a panic and kill the activity subsystem.
    pub fn push(&self, event: ActivityEvent) {
        let mut events = self
            .0
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        events.push_back(event);
        while events.len() > ACTIVITY_CAP {
            events.pop_front();
        }
        self.0.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_opened(&self) {
        self.0.connections.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the active-connection count, saturating at zero so a disconnect
    /// without a matching connect (or a double fire) can never wrap.
    pub fn connection_closed(&self) {
        let _ = self
            .0
            .connections
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                Some(c.saturating_sub(1))
            });
    }

    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::Relaxed)
    }

    pub fn connections(&self) -> usize {
        self.0.connections.load(Ordering::Relaxed)
    }

    /// Snapshot the last `max_events` events plus the current count/generation.
    ///
    /// The generation is read while the events lock is held, so it is always
    /// coherent with the events copied (see [`Self::push`]). The connection
    /// counter is read here too; it is maintained outside this lock, so it is a
    /// best-effort count that can momentarily lead or trail the events by a
    /// frame — acceptable for a status display, and self-corrected within the
    /// next wall-clock-second redraw.
    pub fn snapshot(&self, max_events: usize) -> ActivitySnapshot {
        let events = self
            .0
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let start = events.len().saturating_sub(max_events);
        let tail = events.iter().skip(start).cloned().collect();
        ActivitySnapshot {
            generation: self.0.generation.load(Ordering::Relaxed),
            connections: self.0.connections.load(Ordering::Relaxed),
            events: tail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(msg: &str, tone: ActivityTone) -> ActivityEvent {
        ActivityEvent {
            hms: "00:00:00".to_string(),
            tone,
            message: msg.to_string(),
        }
    }

    #[test]
    fn ring_starts_empty_with_zero_connections() {
        let ring = ActivityRing::new();
        let snap = ring.snapshot(ACTIVITY_CAP);
        assert!(snap.events.is_empty());
        assert_eq!(snap.connections, 0);
        assert_eq!(snap.generation, 0);
    }

    #[test]
    fn push_appends_and_bumps_generation() {
        let ring = ActivityRing::new();
        ring.push(ev("a", ActivityTone::Info));
        ring.push(ev("b", ActivityTone::Ok));
        let snap = ring.snapshot(ACTIVITY_CAP);
        assert_eq!(snap.generation, 2);
        let msgs: Vec<&str> = snap.events.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, vec!["a", "b"], "events preserve insertion order");
    }

    #[test]
    fn ring_caps_at_capacity_dropping_oldest() {
        let ring = ActivityRing::new();
        for n in 0..(ACTIVITY_CAP + 10) {
            ring.push(ev(&format!("line{n}"), ActivityTone::Info));
        }
        let snap = ring.snapshot(ACTIVITY_CAP);
        assert_eq!(snap.events.len(), ACTIVITY_CAP, "never exceeds the cap");
        // The oldest 10 were dropped; the tail is line10..line(CAP+9).
        assert_eq!(snap.events.first().unwrap().message, "line10");
        assert_eq!(
            snap.events.last().unwrap().message,
            format!("line{}", ACTIVITY_CAP + 9)
        );
        // Generation counts every push, including the dropped ones.
        assert_eq!(snap.generation, (ACTIVITY_CAP + 10) as u64);
    }

    #[test]
    fn snapshot_returns_only_the_last_max_events() {
        let ring = ActivityRing::new();
        for n in 0..20 {
            ring.push(ev(&format!("l{n}"), ActivityTone::Info));
        }
        let snap = ring.snapshot(5);
        assert_eq!(snap.events.len(), 5);
        assert_eq!(snap.events.first().unwrap().message, "l15");
        assert_eq!(snap.events.last().unwrap().message, "l19");
    }

    #[test]
    fn connection_counter_increments_and_decrements() {
        let ring = ActivityRing::new();
        ring.connection_opened();
        ring.connection_opened();
        assert_eq!(ring.connections(), 2);
        ring.connection_closed();
        assert_eq!(ring.connections(), 1);
    }

    #[test]
    fn connection_close_saturates_at_zero() {
        let ring = ActivityRing::new();
        // A disconnect with no matching connect (or a double-fire) must never wrap.
        ring.connection_closed();
        ring.connection_closed();
        assert_eq!(ring.connections(), 0);
    }

    #[test]
    fn concurrent_pushes_and_snapshots_stay_consistent() {
        // Many producer threads push while a reader snapshots in a tight loop —
        // the shape of the real workload (tokio workers push, the TUI thread
        // reads). The ring must never exceed the cap, every reader snapshot must
        // carry a generation that is consistent with its events (generation is
        // bumped under the same lock as the push, so a snapshot can never show
        // more events than its generation accounts for), and nothing may panic.
        use std::sync::Arc;
        use std::thread;

        const THREADS: usize = 8;
        const PER_THREAD: usize = 1000;
        let ring = Arc::new(ActivityRing::new());

        let reader = {
            let ring = Arc::clone(&ring);
            thread::spawn(move || {
                for _ in 0..5000 {
                    let snap = ring.snapshot(ACTIVITY_CAP);
                    assert!(snap.events.len() <= ACTIVITY_CAP);
                    // The events copied under the lock can never outnumber the
                    // generation read under that same lock.
                    assert!(snap.events.len() as u64 <= snap.generation);
                }
            })
        };

        let mut producers = Vec::new();
        for t in 0..THREADS {
            let ring = Arc::clone(&ring);
            producers.push(thread::spawn(move || {
                for n in 0..PER_THREAD {
                    ring.push(ev(&format!("t{t}-{n}"), ActivityTone::Info));
                }
            }));
        }
        for p in producers {
            p.join().unwrap();
        }
        reader.join().unwrap();

        let snap = ring.snapshot(ACTIVITY_CAP);
        assert_eq!(snap.events.len(), ACTIVITY_CAP, "ring stays capped");
        assert_eq!(
            snap.generation,
            (THREADS * PER_THREAD) as u64,
            "every push bumped the generation exactly once"
        );
    }
}
