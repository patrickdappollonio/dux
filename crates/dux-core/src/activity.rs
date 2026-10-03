//! A bounded, thread-safe tail of the web server's console lines plus a live
//! active-connection count. The web `Console` (the producer, on many tokio
//! worker threads) pushes here; the flip's server status screen (the consumer,
//! on the engine-loop thread) reads what arrived since it last looked
//! ([`ActivityRing::lines_since`]) and scrolls back through its own copy.
//!
//! The lines are the very [`LogLine`]s `dux server` prints, so the two surfaces
//! cannot word anything differently. The buffer keeps the most recent
//! `capacity` lines (`[server] log_viewer_lines`) and drops the oldest, except
//! the startup lines, which are pinned ([`ActivityRing::pin_startup`]) so a
//! busy server cannot evict its own banner and reachability note.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::serve_log::LogLine;

/// A point-in-time read of the whole log: the line generation, the live
/// connection count, and every line in order (the pinned startup lines, then
/// the retained rest, oldest first).
#[derive(Clone, Debug)]
pub struct ActivitySnapshot {
    /// How many lines were ever pushed, dropped ones included.
    pub generation: u64,
    pub connections: usize,
    pub lines: Vec<LogLine>,
}

#[derive(Default)]
struct Held {
    /// The startup lines, never evicted.
    pinned: Vec<LogLine>,
    /// Everything after them, bounded by the capacity.
    lines: VecDeque<LogLine>,
}

struct ActivityInner {
    held: Mutex<Held>,
    capacity: usize,
    connections: AtomicUsize,
    /// Bumped on every push (including pushes that drop an older line), so a
    /// reader can detect new activity without copying the buffer.
    generation: AtomicU64,
    /// Bumped whenever the pinned lines change, which also moves lines out of
    /// the bounded part: a reader that sees it move reads everything again.
    pin_version: AtomicU64,
    /// Every URL the serve can be opened at right now: its listeners, then this
    /// machine's MagicDNS URL and `tailscale serve` routes. Empty until the
    /// serve publishes them, which the status screen reads as "show the
    /// addresses captured at start".
    serve_urls: Mutex<Vec<String>>,
    /// Bumped when [`Self::serve_urls`] changes. Its own counter, because the
    /// line generation is what counts lines for [`ActivityRing::lines_since`].
    urls_version: AtomicU64,
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
    /// A ring holding at most `capacity` lines past the pinned ones, read
    /// through [`crate::config::log_viewer_capacity`] (at least 1, at most
    /// [`crate::config::LOG_VIEWER_LINES_MAX`]).
    pub fn new(capacity: usize) -> Self {
        let capacity = crate::config::log_viewer_capacity(capacity);
        Self(Arc::new(ActivityInner {
            held: Mutex::new(Held {
                pinned: Vec::new(),
                lines: VecDeque::with_capacity(capacity.min(4096)),
            }),
            capacity,
            connections: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            pin_version: AtomicU64::new(0),
            serve_urls: Mutex::new(Vec::new()),
            urls_version: AtomicU64::new(0),
        }))
    }

    /// The most lines this ring keeps past the pinned ones.
    pub fn capacity(&self) -> usize {
        self.0.capacity
    }

    /// A poisoned lock is recovered rather than propagated: this is a lossy,
    /// display-only buffer, so one panic must not kill the activity subsystem.
    fn held(&self) -> MutexGuard<'_, Held> {
        self.0
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Append a line, dropping the oldest unpinned one if the buffer is full,
    /// then bump the generation, all under the lock so a concurrent reader
    /// cannot see the new line with the old generation and miss a redraw.
    pub fn push(&self, line: LogLine) {
        let mut held = self.held();
        held.lines.push_back(line);
        while held.lines.len() > self.0.capacity {
            held.lines.pop_front();
        }
        self.0.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Pin the startup: every line logged so far (the warnings raised before
    /// binding) and then `banner`, so a full buffer never evicts them.
    pub fn pin_startup(&self, banner: Vec<LogLine>) {
        let mut held = self.held();
        let earlier: Vec<LogLine> = held.lines.drain(..).collect();
        held.pinned.extend(earlier);
        held.pinned.extend(banner);
        self.0.pin_version.fetch_add(1, Ordering::Relaxed);
    }

    /// The pinned startup lines.
    pub fn pinned(&self) -> Vec<LogLine> {
        self.held().pinned.clone()
    }

    /// Changes whenever the pinned lines do.
    pub fn pin_version(&self) -> u64 {
        self.0.pin_version.load(Ordering::Relaxed)
    }

    /// The unpinned lines pushed after generation `since` that are still held,
    /// oldest first, plus the generation they bring the reader up to. Copies
    /// only what is new, which is what keeps a reader's cost per line rather
    /// than per buffer.
    pub fn lines_since(&self, since: u64) -> (u64, Vec<LogLine>) {
        let held = self.held();
        let generation = self.0.generation.load(Ordering::Relaxed);
        let arrived = usize::try_from(generation.saturating_sub(since))
            .unwrap_or(usize::MAX)
            .min(held.lines.len());
        let start = held.lines.len() - arrived;
        (generation, held.lines.range(start..).cloned().collect())
    }

    /// Replace the serve's live URL list. The same list again is no change.
    pub fn set_serve_urls(&self, urls: Vec<String>) {
        let mut slot = self
            .0
            .serve_urls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *slot != urls {
            *slot = urls;
            self.0.urls_version.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The serve's live URL list, empty until the serve has published one.
    pub fn serve_urls(&self) -> Vec<String> {
        self.0
            .serve_urls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Moves whenever [`Self::serve_urls`] changes.
    pub fn urls_version(&self) -> u64 {
        self.0.urls_version.load(Ordering::Relaxed)
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

    /// The whole log, pinned lines first. Copies everything, so it is for tests
    /// and one-off reads, never a per-frame path.
    pub fn snapshot(&self) -> ActivitySnapshot {
        let held = self.held();
        ActivitySnapshot {
            generation: self.0.generation.load(Ordering::Relaxed),
            connections: self.0.connections.load(Ordering::Relaxed),
            lines: held
                .pinned
                .iter()
                .chain(held.lines.iter())
                .cloned()
                .collect(),
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

    fn texts_of(lines: &[LogLine]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.text()
                    .trim_start_matches("00:00:00 \u{279c} ")
                    .to_string()
            })
            .collect()
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
    fn the_live_url_list_has_its_own_version_and_never_moves_the_line_count() {
        let ring = ActivityRing::new(10);
        assert!(ring.serve_urls().is_empty());
        let before = (ring.generation(), ring.urls_version());
        let urls = vec!["http://127.0.0.1:3890".to_string()];
        ring.set_serve_urls(urls.clone());
        assert_eq!(ring.serve_urls(), urls);
        assert_eq!(
            ring.generation(),
            before.0,
            "lines_since counts lines by it"
        );
        assert_eq!(ring.urls_version(), before.1 + 1);
        ring.set_serve_urls(urls);
        assert_eq!(
            ring.urls_version(),
            before.1 + 1,
            "the same list is no change"
        );
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

    /// The startup lines (its warnings and the banner, with the reachability
    /// note) are pinned: however much the server logs afterwards, a full buffer
    /// never evicts them.
    #[test]
    fn pinned_startup_lines_survive_a_full_buffer() {
        let ring = ActivityRing::new(3);
        ring.push(line("startup warning"));
        ring.pin_startup(vec![line("banner")]);
        for n in 0..10 {
            ring.push(line(&format!("event{n}")));
        }
        assert_eq!(texts_of(&ring.pinned()), vec!["startup warning", "banner"]);
        assert_eq!(
            texts(&ring.snapshot()),
            vec!["startup warning", "banner", "event7", "event8", "event9"],
            "a snapshot is the whole log in order: pinned lines, then the rest"
        );
    }

    #[test]
    fn lines_since_hands_over_only_what_arrived() {
        let ring = ActivityRing::new(10);
        ring.push(line("a"));
        let seen = ring.generation();
        ring.push(line("b"));
        ring.push(line("c"));
        let (generation, new) = ring.lines_since(seen);
        assert_eq!(generation, 3);
        assert_eq!(texts_of(&new), vec!["b", "c"]);
        assert!(ring.lines_since(generation).1.is_empty());
    }

    #[test]
    fn lines_since_hands_over_only_what_is_still_held() {
        let ring = ActivityRing::new(2);
        for n in 0..5 {
            ring.push(line(&format!("l{n}")));
        }
        let (_, new) = ring.lines_since(0);
        assert_eq!(texts_of(&new), vec!["l3", "l4"]);
    }

    #[test]
    fn a_capacity_above_the_maximum_is_read_as_the_maximum() {
        let ring = ActivityRing::new(usize::MAX);
        assert_eq!(ring.capacity(), crate::config::LOG_VIEWER_LINES_MAX);
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
