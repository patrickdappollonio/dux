use indexmap::IndexMap;
use std::time::{Duration, Instant};

/// Shared timeout for upgrading stale `Busy` entries to `Warning`. Used by
/// both the TUI tick and the web engine actor so the behaviour is identical on
/// both surfaces and the value only lives in one place.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Info,
    Busy,
    Warning,
    Error,
}

impl StatusTone {
    /// The wire tone string shared with the web client (matches `WireStatus`).
    pub fn as_wire(self) -> &'static str {
        match self {
            StatusTone::Info => "info",
            StatusTone::Busy => "busy",
            StatusTone::Warning => "warning",
            StatusTone::Error => "error",
        }
    }

    /// Parse a wire tone string back to a tone; an unknown tone maps to `Info`
    /// (the neutral default), matching how the web client treats it.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "busy" => StatusTone::Busy,
            "warning" => StatusTone::Warning,
            "error" => StatusTone::Error,
            _ => StatusTone::Info,
        }
    }

    /// Whether a status of this tone auto-clears after the timeout. Only `Info`
    /// (which dux uses for success/positive confirmations) expires: `Busy` waits
    /// for the final state that replaces it, and `Warning`/`Error` persist until
    /// the next status so a problem is never missed.
    fn auto_clears(self) -> bool {
        matches!(self, StatusTone::Info)
    }
}

/// Braille spinner frames, shared between the TUI render path and the keyed
/// controller's `most_recent()` result.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Return the braille spinner frame appropriate for the given wall-clock
/// `since` instant (advances every 100 ms). Used by the TUI's `render_footer`
/// when displaying a `Busy` status from the keyed controller.
pub fn spinner_frame_for(since: Instant) -> &'static str {
    let index = ((since.elapsed().as_millis() / 100) as usize) % SPINNER_FRAMES.len();
    SPINNER_FRAMES[index]
}

// ---------------------------------------------------------------------------
// Keyed multi-status controller
// ---------------------------------------------------------------------------

/// A monotonic per-key generation token. A producer that re-emits on the same
/// key bumps the token; a clear/success only removes the entry when the token it
/// carries MATCHES the stored one, so a stale success can never dismiss a newer
/// status that a concurrent retry placed on the same key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Generation(pub u64);

/// One open status, keyed or anonymous.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyedStatus {
    /// `None` = the anonymous slot (unkeyed transients); `Some` = a keyed op.
    pub key: Option<String>,
    pub tone: StatusTone,
    pub message: String,
    pub generation: Generation,
    /// Wall-clock time when this status was last set. Used for auto-clear and
    /// busy-timeout decisions in `tick`.
    since: Instant,
    /// Monotonic insertion counter for `most_recent()` disambiguation when two
    /// entries share the same `since` timestamp.
    seq: u64,
}

/// The wire-safe projection of one open keyed status (snapshot + broadcast).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyedWireStatus {
    pub key: Option<String>,
    pub tone: String, // StatusTone::as_wire()
    pub message: String,
}

/// What `tick` changed, so the web actor can broadcast precise StatusCleared /
/// status frames and the TUI can re-render.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusTickChanges {
    pub cleared_keys: Vec<Option<String>>, // None = the anonymous slot cleared
    pub upgraded: Vec<KeyedWireStatus>,    // busy→warning replacements
}

/// A keyed multi-status controller.
///
/// Holds one anonymous slot (for unkeyed transient messages) and a
/// `String → KeyedStatus` map for named operations. Each emit bumps a
/// generation token on its key so that a stale-success clear from a prior
/// attempt can never silently dismiss a newer, live status.
pub struct KeyedStatusController {
    /// The anonymous slot; most-recent-wins.
    anon: Option<KeyedStatus>,
    /// Named entries in insertion order.
    entries: IndexMap<String, KeyedStatus>,
    clear_after: Duration,
    /// Monotonic counter incremented on every `set` call. Used to order entries
    /// when two share the same `since` timestamp.
    next_seq: u64,
    /// Monotonic generation counter incremented for every `set` call.
    next_gen: u64,
    /// When `true` the anonymous slot is exempt from auto-clear even if its
    /// tone would normally expire. Used for the TUI's first-run hint so it
    /// persists until the user's first action replaces it. Any later `set` on
    /// the anonymous slot clears the pin.
    anon_pinned: bool,
}

impl KeyedStatusController {
    pub fn with_clear_after(clear_after: Duration) -> Self {
        Self {
            anon: None,
            entries: IndexMap::new(),
            clear_after,
            next_seq: 0,
            next_gen: 0,
            anon_pinned: false,
        }
    }

    /// Exempt the CURRENT anonymous-slot message from auto-clear. Used for the
    /// TUI's first-run help hint so it persists until the user's first action
    /// replaces it. A subsequent anonymous `set` clears the pin.
    pub fn pin(&mut self) {
        self.anon_pinned = true;
    }

    pub fn set_clear_after(&mut self, clear_after: Duration) {
        self.clear_after = clear_after;
    }

    /// Set/replace a status.
    ///
    /// - `key == None` writes the anonymous slot (most-recent-wins).
    /// - `key == Some(_)` upserts the named entry and bumps its generation.
    ///
    /// Returns the stored entry's generation so a producer can correlate a
    /// later explicit clear.
    pub fn set(
        &mut self,
        now: Instant,
        key: Option<String>,
        tone: StatusTone,
        message: impl Into<String>,
    ) -> Generation {
        let generation = Generation(self.next_gen);
        let seq = self.next_seq;
        self.next_gen += 1;
        self.next_seq += 1;

        let entry = KeyedStatus {
            key: key.clone(),
            tone,
            message: message.into(),
            generation,
            since: now,
            seq,
        };

        match key {
            None => {
                self.anon = Some(entry);
                // A new anonymous set always clears the pin so the new message
                // follows normal auto-clear rules (the pin was for the old one).
                self.anon_pinned = false;
            }
            Some(k) => {
                self.entries.insert(k, entry);
            }
        }

        generation
    }

    /// Remove a keyed entry IFF the carried generation matches the stored one
    /// (the clear-race guard).
    ///
    /// - `generation == None` clears unconditionally (used by the auto-clear
    ///   tick for expired Info entries).
    ///
    /// Returns `true` if anything was removed.
    pub fn clear(&mut self, key: &str, generation: Option<Generation>) -> bool {
        if let Some(entry) = self.entries.get(key) {
            let matches = match generation {
                None => true,
                Some(g) => entry.generation == g,
            };
            if matches {
                self.entries.swap_remove(key);
                return true;
            }
        }
        false
    }

    /// Expire timed-out entries.
    ///
    /// - `Info`/success entries older than `clear_after` are removed.
    /// - `Busy` entries older than `busy_timeout` are upgraded in-place to a
    ///   `Warning` with a "timed out" message, so a leaked busy is never
    ///   immortal.
    ///
    /// Returns the set of changes the caller must broadcast.
    pub fn tick(&mut self, now: Instant, busy_timeout: Duration) -> StatusTickChanges {
        let mut changes = StatusTickChanges::default();

        // Check the anonymous slot.
        let anon_expired = self.anon.as_ref().is_some_and(|a| {
            !self.anon_pinned
                && a.tone.auto_clears()
                && !self.clear_after.is_zero()
                && now.duration_since(a.since) >= self.clear_after
        });
        if anon_expired {
            self.anon = None;
            changes.cleared_keys.push(None);
        }

        // Collect keys to operate on; two passes to avoid borrow issues.
        let mut to_clear: Vec<String> = Vec::new();
        let mut to_upgrade: Vec<String> = Vec::new();

        for (key, entry) in &self.entries {
            let age = now.duration_since(entry.since);
            match entry.tone {
                StatusTone::Info if !self.clear_after.is_zero() && age >= self.clear_after => {
                    to_clear.push(key.clone());
                }
                StatusTone::Busy if age >= busy_timeout => {
                    to_upgrade.push(key.clone());
                }
                _ => {}
            }
        }

        for key in to_clear {
            self.entries.swap_remove(&key);
            changes.cleared_keys.push(Some(key));
        }

        for key in to_upgrade {
            if let Some(entry) = self.entries.get_mut(&key) {
                // A keyed Busy that reaches the timeout is a leak: its producer
                // emitted a pending status but never the paired final. Log the
                // key and the original message so the unpaired operation is
                // diagnosable in dux.log instead of vanishing into a generic
                // "timed out" warning. (See the StatusOp design: every Busy must
                // be followed by a success/error/clear on the same key.)
                crate::logger::warn(&format!(
                    "status key \"{key}\" left Busy with no final (\"{}\"); upgrading to a timed-out warning",
                    entry.message
                ));
                let generation = Generation(self.next_gen);
                let seq = self.next_seq;
                self.next_gen += 1;
                self.next_seq += 1;
                entry.tone = StatusTone::Warning;
                entry.message = "timed out — check dux.log".to_string();
                entry.generation = generation;
                entry.since = now;
                entry.seq = seq;
                changes.upgraded.push(KeyedWireStatus {
                    key: Some(key.clone()),
                    tone: StatusTone::Warning.as_wire().to_string(),
                    message: entry.message.clone(),
                });
            }
        }

        changes
    }

    /// All open statuses (anonymous slot first if present, then keyed entries
    /// in insertion order), for the reconnect snapshot.
    pub fn snapshot(&self) -> Vec<KeyedWireStatus> {
        let mut out = Vec::new();
        if let Some(ref anon) = self.anon {
            out.push(KeyedWireStatus {
                key: None,
                tone: anon.tone.as_wire().to_string(),
                message: anon.message.clone(),
            });
        }
        for entry in self.entries.values() {
            out.push(KeyedWireStatus {
                key: Some(entry.key.clone().unwrap_or_default()),
                tone: entry.tone.as_wire().to_string(),
                message: entry.message.clone(),
            });
        }
        out
    }

    /// The single line the TUI shows: the most-recently-set open status (keyed
    /// or anonymous), or `None` when nothing is open.
    ///
    /// When two entries share the same `since` timestamp the one with the
    /// higher sequence number wins (the later `set` call).
    pub fn most_recent(&self) -> Option<KeyedWireStatus> {
        let anon_ref = self.anon.as_ref();
        let keyed_ref = self.entries.values().max_by_key(|e| (e.since, e.seq));

        let winner = match (anon_ref, keyed_ref) {
            (None, None) => return None,
            (Some(a), None) => a,
            (None, Some(k)) => k,
            (Some(a), Some(k)) => {
                if (a.since, a.seq) >= (k.since, k.seq) {
                    a
                } else {
                    k
                }
            }
        };

        Some(KeyedWireStatus {
            key: winner.key.clone(),
            tone: winner.tone.as_wire().to_string(),
            message: winner.message.clone(),
        })
    }

    /// Whether the anonymous (unkeyed) slot currently holds a `Busy` entry
    /// with the exact given message. Used by deletion workers to guard against
    /// clobbering a newer status that replaced their Busy while they ran.
    pub fn anon_busy_matches(&self, message: &str) -> bool {
        self.anon
            .as_ref()
            .is_some_and(|a| a.tone == StatusTone::Busy && a.message == message)
    }

    /// TUI projection: the most-recently-set open status as a `(tone, text)`
    /// pair suitable for direct rendering. For `Busy` entries the braille
    /// spinner is prepended exactly as [`StatusLine::text()`] does, using the
    /// entry's `since` instant so the animation stays wall-clock based.
    /// Returns `None` when no status is open.
    pub fn most_recent_tui(&self) -> Option<(StatusTone, String)> {
        let anon_ref = self.anon.as_ref();
        let keyed_ref = self.entries.values().max_by_key(|e| (e.since, e.seq));

        let winner = match (anon_ref, keyed_ref) {
            (None, None) => return None,
            (Some(a), None) => a,
            (None, Some(k)) => k,
            (Some(a), Some(k)) => {
                if (a.since, a.seq) >= (k.since, k.seq) {
                    a
                } else {
                    k
                }
            }
        };

        let text = match winner.tone {
            StatusTone::Busy => {
                format!("{} {}", spinner_frame_for(winner.since), winner.message)
            }
            _ => winner.message.clone(),
        };
        Some((winner.tone, text))
    }

    // -----------------------------------------------------------------------
    // Single-status compatibility surface — thin wrappers over the most-recent
    // projection used by TUI tests and existing call sites.
    // -----------------------------------------------------------------------

    /// The tone of the most-recently-set open status, or `Info` when nothing
    /// is open (mirrors the previous `StatusLine::tone()` API).
    pub fn tone(&self) -> StatusTone {
        self.most_recent_tui()
            .map(|(t, _)| t)
            .unwrap_or(StatusTone::Info)
    }

    /// The rendered text of the most-recently-set open status (spinner
    /// prepended for `Busy`), or an empty string when nothing is open.
    /// Mirrors the previous `StatusLine::text()` API.
    pub fn text(&self) -> String {
        self.most_recent_tui().map(|(_, t)| t).unwrap_or_default()
    }

    /// The raw message of the most-recently-set open status without any
    /// spinner prefix, or an empty string when nothing is open. Mirrors the
    /// previous `StatusLine::message()` API.
    pub fn message(&self) -> String {
        let anon_ref = self.anon.as_ref();
        let keyed_ref = self.entries.values().max_by_key(|e| (e.since, e.seq));
        let winner = match (anon_ref, keyed_ref) {
            (None, None) => return String::new(),
            (Some(a), None) => a,
            (None, Some(k)) => k,
            (Some(a), Some(k)) => {
                if (a.since, a.seq) >= (k.since, k.seq) {
                    a
                } else {
                    k
                }
            }
        };
        winner.message.clone()
    }

    /// Whether no status is currently open. Mirrors the previous
    /// `StatusLine::is_empty()` API.
    pub fn is_empty(&self) -> bool {
        self.anon.is_none() && self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyedStatusController, StatusTone};
    use std::time::{Duration, Instant};

    #[test]
    fn wire_tone_round_trips() {
        for tone in [
            StatusTone::Info,
            StatusTone::Busy,
            StatusTone::Warning,
            StatusTone::Error,
        ] {
            assert_eq!(StatusTone::from_wire(tone.as_wire()), tone);
        }
        // Unknown tones fall back to Info.
        assert_eq!(StatusTone::from_wire("nonsense"), StatusTone::Info);
    }

    // -----------------------------------------------------------------------
    // KeyedStatusController tests
    // -----------------------------------------------------------------------

    #[test]
    fn keyed_clear_only_fires_on_matching_generation() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::from_secs(6));
        // First emit on "pull" (busy).
        let g1 = c.set(t0, Some("pull".into()), StatusTone::Busy, "Pulling…");
        // A concurrent retry replaces it (new generation).
        let g2 = c.set(t0, Some("pull".into()), StatusTone::Error, "Pull failed.");
        assert_ne!(g1, g2, "re-emit must bump the generation");
        // The STALE success (g1) must NOT dismiss the newer error (g2).
        assert!(
            !c.clear("pull", Some(g1)),
            "stale-gen clear must be ignored"
        );
        assert_eq!(c.most_recent().unwrap().tone, "error");
        // The matching clear (g2) removes it.
        assert!(c.clear("pull", Some(g2)));
        assert!(c.most_recent().is_none());
    }

    #[test]
    fn keyed_busy_expires_to_warning_after_timeout() {
        let t0 = Instant::now();
        let busy_timeout = Duration::from_secs(20);
        let mut c = KeyedStatusController::with_clear_after(Duration::from_secs(6));
        c.set(
            t0,
            Some("launch".into()),
            StatusTone::Busy,
            "Launching agent…",
        );
        // Before the bound: untouched.
        let changes = c.tick(t0 + Duration::from_secs(19), busy_timeout);
        assert!(changes.upgraded.is_empty());
        assert_eq!(c.most_recent().unwrap().tone, "busy");
        // After the bound: upgraded to warning IN PLACE, broadcast in `upgraded`.
        let changes = c.tick(t0 + Duration::from_secs(20), busy_timeout);
        assert_eq!(changes.upgraded.len(), 1);
        assert_eq!(changes.upgraded[0].key.as_deref(), Some("launch"));
        assert_eq!(changes.upgraded[0].tone, "warning");
        let mr = c.most_recent().unwrap();
        assert_eq!(mr.tone, "warning");
        assert!(mr.message.to_lowercase().contains("timed out"));
    }

    #[test]
    fn keyed_info_auto_clears_anonymous_and_keyed() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::from_secs(6));
        c.set(t0, None, StatusTone::Info, "Saved.");
        c.set(t0, Some("commit".into()), StatusTone::Info, "Committed.");
        // Warning/error do not auto-clear.
        c.set(t0, Some("acme".into()), StatusTone::Error, "ACME error.");
        let changes = c.tick(t0 + Duration::from_secs(6), Duration::from_secs(20));
        // Both Info entries cleared; the error persists.
        assert_eq!(changes.cleared_keys.len(), 2);
        assert!(changes.cleared_keys.contains(&None));
        assert!(changes.cleared_keys.contains(&Some("commit".to_string())));
        assert_eq!(c.snapshot().len(), 1);
        assert_eq!(c.snapshot()[0].key.as_deref(), Some("acme"));
    }

    #[test]
    fn tui_keyed_clear_dismisses_the_line() {
        // Verifies the TUI most-recent-wins projection: a matching keyed clear
        // removes the entry so the TUI line becomes empty (Task 11).
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::ZERO);
        let g = c.set(t0, Some("pull".into()), StatusTone::Busy, "Pulling\u{2026}");
        assert!(c.most_recent().is_some());
        assert!(c.clear("pull", Some(g)));
        assert!(
            c.most_recent().is_none(),
            "a matching clear must empty the TUI line"
        );
    }

    #[test]
    fn tui_most_recent_tui_prepends_spinner_for_busy() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::ZERO);
        c.set(t0, None, StatusTone::Busy, "Pulling\u{2026}");
        let (tone, text) = c.most_recent_tui().expect("should have a status");
        assert_eq!(tone, StatusTone::Busy);
        // The spinner is one braille char followed by a space and the message.
        assert!(
            text.starts_with(['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏']),
            "expected spinner prefix, got: {text:?}"
        );
        assert!(
            text.ends_with("Pulling\u{2026}"),
            "message must be in text: {text:?}"
        );
    }

    #[test]
    fn tui_anon_pin_survives_tick_but_clears_on_new_set() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::from_secs(6));
        c.set(t0, None, StatusTone::Info, "Press ? for help");
        c.pin();
        // Pinned anon slot must NOT auto-clear even well past the timeout.
        let changes = c.tick(t0 + Duration::from_secs(3600), Duration::from_secs(20));
        assert!(
            changes.cleared_keys.is_empty(),
            "pinned anon slot must not auto-clear"
        );
        assert!(c.most_recent().is_some());
        // A new set on the anonymous slot resets the pin and resumes normal rules.
        c.set(
            t0 + Duration::from_secs(3600),
            None,
            StatusTone::Info,
            "Saved.",
        );
        let changes = c.tick(t0 + Duration::from_secs(3607), Duration::from_secs(20));
        assert_eq!(
            changes.cleared_keys,
            vec![None],
            "after a new set the pin is gone and auto-clear must fire"
        );
        assert!(c.most_recent().is_none());
    }

    #[test]
    fn snapshot_lists_every_open_status_for_reconnect() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::ZERO); // no auto-clear
        c.set(t0, Some("pull".into()), StatusTone::Busy, "Pulling…");
        c.set(t0, Some("launch".into()), StatusTone::Busy, "Launching…");
        c.set(t0, None, StatusTone::Warning, "Heads up.");
        let snap = c.snapshot();
        assert_eq!(
            snap.len(),
            3,
            "every open status must appear in the snapshot"
        );
    }
}
