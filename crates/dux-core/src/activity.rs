//! A bounded, thread-safe tail of the web server's console lines plus a live
//! active-connection count. The web `Console` (the producer, on many tokio
//! worker threads) pushes here; the flip's server status screen (the consumer,
//! on the engine-loop thread) reads a [`ActivityRing::snapshot`] when it redraws
//! and scrolls back through it.
//!
//! The lines are the very [`LogLine`]s `dux server` prints, so the two surfaces
//! cannot word anything differently. The buffer keeps the most recent
//! `capacity` lines (`[server] log_viewer_lines`) and drops the oldest.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::serve_log::LogLine;

/// A point-in-time read of the ring: the line generation (for cheap "did
/// anything change?" checks), the live connection count, and every retained
/// line, oldest first.
#[derive(Clone, Debug)]
pub struct ActivitySnapshot {
    /// How many lines were ever pushed, dropped ones included. The difference
    /// between two snapshots' generations is how many lines arrived between
    /// them.
    pub generation: u64,
    pub connections: usize,
    pub lines: Vec<LogLine>,
}

struct ActivityInner {
    lines: Mutex<VecDeque<LogLine>>,
    capacity: usize,
    connections: AtomicUsize,
    /// Bumped on every push (including pushes that drop an older line), so a
    /// reader can detect new activity without copying the buffer.
    generation: AtomicU64,
}

/// A cheap-to-clone (`Arc`) shared handle to the line buffer.
#[derive(Clone)]
pub struct ActivityRing(Arc<ActivityInner>);

impl Default for ActivityRing {
    fn default() -> Self {
        Self::new(crate::config::DEFAULT_LOG_VIEWER_LINES)
    }
}

impl ActivityRing {
    /// A ring holding at most `capacity` lines. A capacity below 1 is read as 1.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self(Arc::new(ActivityInner {
            lines: Mutex::new(VecDeque::with_capacity(capacity.min(4096))),
            capacity,
            connections: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
        }))
    }

    /// The most lines this ring keeps.
    pub fn capacity(&self) -> usize {
        self.0.capacity
    }

    /// Append a line, dropping the oldest if the buffer is full, then bump the
    /// generation, all while holding the lock so a concurrent
    /// [`Self::snapshot`] cannot observe the new line with the old generation
    /// and miss a redraw.
    ///
    /// A poisoned lock is recovered rather than propagated: this is a lossy,
    /// display-only buffer, so one panic must not kill the activity subsystem.
    pub fn push(&self, line: LogLine) {
        let mut lines = self
            .0
            .lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lines.push_back(line);
        while lines.len() > self.0.capacity {
            lines.pop_front();
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
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                Some(c.saturating_sub(1))
            });
    }

    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::Relaxed)
    }

    pub fn connections(&self) -> usize {
        self.0.connections.load(Ordering::Relaxed)
    }

    /// Snapshot every retained line plus the current count/generation.
    ///
    /// The generation is read while the lines lock is held, so it is always
    /// coherent with the lines copied (see [`Self::push`]). The connection
    /// counter is maintained outside that lock, so it is best-effort and may lead
    /// or trail the lines by a frame.
    pub fn snapshot(&self) -> ActivitySnapshot {
        let lines = self
            .0
            .lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ActivitySnapshot {
            generation: self.0.generation.load(Ordering::Relaxed),
            connections: self.0.connections.load(Ordering::Relaxed),
            lines: lines.iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve_log::LogTone;

    fn line(msg: &str) -> LogLine {
        LogLine::event("00:00:00", LogTone::Info, msg)
    }

    fn texts(snap: &ActivitySnapshot) -> Vec<String> {
        snap.lines
            .iter()
            .map(|l| {
                l.text()
                    .trim_start_matches("00:00:00 \u{279c} ")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn ring_starts_empty_with_zero_connections() {
        let ring = ActivityRing::new(10);
        let snap = ring.snapshot();
        assert!(snap.lines.is_empty());
        assert_eq!(snap.connections, 0);
        assert_eq!(snap.generation, 0);
    }

    #[test]
    fn push_appends_and_bumps_generation() {
        let ring = ActivityRing::new(10);
        ring.push(line("a"));
        ring.push(line("b"));
        let snap = ring.snapshot();
        assert_eq!(snap.generation, 2);
        assert_eq!(texts(&snap), vec!["a", "b"], "lines keep insertion order");
    }

    #[test]
    fn ring_keeps_its_configured_capacity_dropping_oldest() {
        let ring = ActivityRing::new(5);
        for n in 0..8 {
            ring.push(line(&format!("line{n}")));
        }
        let snap = ring.snapshot();
        assert_eq!(
            texts(&snap),
            vec!["line3", "line4", "line5", "line6", "line7"]
        );
        assert_eq!(snap.generation, 8, "generation counts dropped pushes too");
    }

    #[test]
    fn a_capacity_below_one_is_read_as_one() {
        let ring = ActivityRing::new(0);
        ring.push(line("a"));
        ring.push(line("b"));
        assert_eq!(ring.capacity(), 1);
        assert_eq!(texts(&ring.snapshot()), vec!["b"]);
    }

    #[test]
    fn the_default_ring_holds_the_default_viewer_lines() {
        assert_eq!(
            ActivityRing::default().capacity(),
            crate::config::DEFAULT_LOG_VIEWER_LINES
        );
    }

    #[test]
    fn connection_counter_increments_and_decrements() {
        let ring = ActivityRing::new(10);
        ring.connection_opened();
        ring.connection_opened();
        assert_eq!(ring.connections(), 2);
        ring.connection_closed();
        assert_eq!(ring.connections(), 1);
    }

    #[test]
    fn connection_close_saturates_at_zero() {
        let ring = ActivityRing::new(10);
        ring.connection_closed();
        ring.connection_closed();
        assert_eq!(ring.connections(), 0);
    }

    #[test]
    fn concurrent_pushes_and_snapshots_stay_consistent() {
        // Producers push while a reader snapshots in a tight loop, the shape of
        // the real workload. The ring never exceeds its capacity and a snapshot
        // never shows more lines than its generation accounts for.
        use std::thread;

        const CAP: usize = 50;
        const THREADS: usize = 8;
        const PER_THREAD: usize = 1000;
        let ring = Arc::new(ActivityRing::new(CAP));

        let reader = {
            let ring = Arc::clone(&ring);
            thread::spawn(move || {
                for _ in 0..5000 {
                    let snap = ring.snapshot();
                    assert!(snap.lines.len() <= CAP);
                    assert!(snap.lines.len() as u64 <= snap.generation);
                }
            })
        };

        let mut producers = Vec::new();
        for t in 0..THREADS {
            let ring = Arc::clone(&ring);
            producers.push(thread::spawn(move || {
                for n in 0..PER_THREAD {
                    ring.push(line(&format!("t{t}-{n}")));
                }
            }));
        }
        for p in producers {
            p.join().unwrap();
        }
        reader.join().unwrap();

        let snap = ring.snapshot();
        assert_eq!(snap.lines.len(), CAP, "ring stays capped");
        assert_eq!(snap.generation, (THREADS * PER_THREAD) as u64);
    }
}
