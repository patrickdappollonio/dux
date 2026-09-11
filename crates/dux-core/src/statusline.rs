use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Audience for a status update. `All` (the default) broadcasts to every
/// connected client; `Connection(id)` restricts delivery to the single web
/// connection whose command originated the operation, so one client's operation
/// toasts (push, commit, launch) stay off every other client.
///
/// Carried from a status's creation all the way to the wire ([`WireStatus`]).
/// The TUI ignores it entirely (it has one status line and one user); only the
/// web's per-connection status forwarder filters on it. Spontaneous engine
/// statuses and TUI-minted ones default to `All`.
///
/// [`WireStatus`]: crate::wire::WireStatus
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusScope {
    /// Broadcast to every client.
    #[default]
    All,
    /// Deliver only to the web connection with this server-assigned id.
    Connection(String),
}

/// Which surfaces withhold a status from the user.
///
/// A confirmation is owed wherever the screen cannot vouch for the outcome, and
/// the two surfaces answer that separately: the web can quiet a message the
/// terminal still needs, because a browser has affordances a status line has
/// not. Every quieted status still rides back to its caller as the command's
/// answer, so the sentence survives for an API client and for the log.
///
/// Only an INFO is ever withheld. A warning, an error and a spinner all report
/// something the screen cannot be standing in for, so both gates ignore this on
/// them. A quieted INFO that carries a KEY still retires the operation behind
/// it: the spinner goes away and only the sentence is withheld, and when the
/// busy refuses to go (a sticky one waits for the user) the sentence is shown
/// instead, because a spinner nothing retires is worse than a message nobody
/// needed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuietSurfaces {
    /// Withheld from the web's toasts.
    pub web: bool,
    /// Withheld from the terminal UI's status line.
    pub tui: bool,
}

impl QuietSurfaces {
    /// Shown on both surfaces. The default, and what every status that does not
    /// say otherwise is.
    pub const LOUD: Self = Self {
        web: false,
        tui: false,
    };
    /// Withheld from the web only.
    pub const WEB: Self = Self {
        web: true,
        tui: false,
    };
    /// Withheld from the terminal UI only.
    pub const TUI: Self = Self {
        web: false,
        tui: true,
    };
    /// Withheld from both surfaces.
    pub const BOTH: Self = Self {
        web: true,
        tui: true,
    };

    /// Whether this status is shown everywhere.
    pub fn is_loud(&self) -> bool {
        !self.web && !self.tui
    }

    /// Whether the web shows this status. The wire omits the `quiet` field
    /// entirely when it does, so the shape a browser reads is what it was
    /// before the flag grew a second half.
    pub fn shows_on_web(&self) -> bool {
        !self.web
    }
}

/// Shared timeout for upgrading stale `Busy` entries to `Warning`. Used by
/// both the TUI tick and the web engine actor so the behaviour is identical on
/// both surfaces and the value only lives in one place.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(20);

/// Absolute ceiling on how long liveness may keep a `Busy` on screen.
///
/// [`LiveStatusKeys`] is a registry, and a registry can leak: an operation whose
/// final never lands would otherwise hold a spinner on screen for the life of
/// the process, so past this age a `Busy` is upgraded whatever liveness says.
///
/// Generous on purpose. It is a backstop for a bug, not a timeout: a value a
/// slow clone or a long fetch could plausibly cross would reintroduce the false
/// "timed out" this mechanism exists to remove.
pub const BUSY_LIVE_CEILING: Duration = Duration::from_secs(30 * 60);

/// The keys of status operations the engine is still running.
///
/// The busy timeout exists to stop a leaked spinner claiming forever that work
/// is happening, and from inside [`KeyedStatusController`] it can only ever be a
/// guess about silence: nothing there knows whether a clone on a slow network is
/// thirty seconds into its work or was abandoned, and guessing wrong replaces a
/// truthful spinner with "timed out".
///
/// So liveness is recorded rather than inferred, in one set rather than per
/// registry. A cheaply cloned handle is shared between the engine, which
/// registers a key when it starts the operation behind it, and the surface's
/// controller, which retires it at the one place a final lands. A final of any
/// origin therefore retires liveness through the same door, and no op registry
/// has to be enumerated anywhere.
///
/// Both halves fail safely if a future path forgets one:
/// - forgetting to register gets the old behavior back for that operation (a
///   false "timed out" after [`BUSY_TIMEOUT`]), and
/// - forgetting to retire is bounded by [`BUSY_LIVE_CEILING`].
#[derive(Clone, Default)]
pub struct LiveStatusKeys(Arc<Mutex<HashSet<String>>>);

impl LiveStatusKeys {
    /// Record that an operation is running behind `key`. Idempotent.
    pub fn register(&self, key: &str) {
        self.with(|set| {
            set.insert(key.to_string());
        });
    }

    /// Record that nothing is running behind `key` any more. Idempotent, and
    /// deliberately tolerant of a key that was never registered: the controller
    /// calls it on every keyed final, most of which never had an op.
    pub fn retire(&self, key: &str) {
        self.with(|set| {
            set.remove(key);
        });
    }

    pub fn is_live(&self, key: &str) -> bool {
        self.with(|set| set.contains(key))
    }

    /// How many operations are registered. For tests and diagnostics; a leak
    /// shows up here as a number that only grows.
    pub fn len(&self) -> usize {
        self.with(|set| set.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A poisoned lock is recovered rather than propagated: this set is an
    /// optimisation over guessing, and a panic in an unrelated thread must not
    /// take the status line down with it.
    fn with<T>(&self, f: impl FnOnce(&mut HashSet<String>) -> T) -> T {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }
}

impl std::fmt::Debug for LiveStatusKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("LiveStatusKeys")
            .field(&self.with(|set| {
                let mut keys: Vec<&str> = set.iter().map(String::as_str).collect();
                keys.sort_unstable();
                keys.join(", ")
            }))
            .finish()
    }
}

/// How long a final stays replayable under [`StatusRetention::Emit`].
///
/// A reconnect is the same user who watched the spinner start, so it must be
/// told how the operation ended; a page load an hour later is a different
/// session and must not be. This window separates the two.
///
/// Deliberately a fixed constant and not `ui.status_clear_seconds`: how long a
/// final stays replayable and how long a toast stays on screen are different
/// questions, and a user who sets that setting to `0` would get unbounded
/// retention.
///
/// The value covers the browser's whole reconnect budget in
/// `crates/dux-web/web/src/lib/reconnectingSocket.ts`, whose last attempt starts
/// a few seconds after a drop, with an order of magnitude to spare. A brand-new
/// tab opened inside the window also sees the final, which is accepted: a
/// thirty-second-old outcome is current.
pub const FINAL_REPLAY_WINDOW: Duration = Duration::from_secs(30);

/// How many `ui.status_clear_seconds` windows a `Warning` stays up, relative to
/// the single window an `Info` gets.
///
/// Must stay equal to `WARNING_DURATION_FACTOR` in
/// `crates/dux-web/web/src/lib/notify.ts` so the status line and the browser
/// toast agree; `the_web_mirrors_the_warning_clear_factor` reads that file and
/// fails if they drift.
pub const WARNING_CLEAR_FACTOR: u32 = 3;

/// How many statuses may wait behind the one on the TUI's status line.
///
/// The line is a queue, not a transcript: a user who looked away for a minute
/// must not come back to a backlog of news that is no longer true, and a runaway
/// producer must not take unbounded memory through the status line.
///
/// When the queue is full the oldest waiting `Info` is dropped, not the newest
/// arrival, because the newer message is the more current fact.
pub const MAX_QUEUED_STATUSES: usize = 5;

/// The storage key of the `n`th anonymous entry under [`StatusRetention::Retain`].
///
/// The TUI queues unkeyed messages instead of overwriting a single slot, so each
/// one needs an identity of its own to hold a queue position. The prefix is a
/// control character no engine status key contains, so a synthetic id can never
/// collide with a real one.
fn anon_storage_key(n: u64) -> String {
    format!("\u{1}anon:{n}")
}

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

    /// How long a status of this tone stays up, as a multiple of the
    /// auto-clear window. `None` means it stays until something replaces it:
    /// `Busy` waits for its final, and an `Error` is the one outcome the user
    /// must not be able to miss by looking away.
    fn clear_windows(self) -> Option<u32> {
        match self {
            StatusTone::Info => Some(1),
            StatusTone::Warning => Some(WARNING_CLEAR_FACTOR),
            StatusTone::Busy | StatusTone::Error => None,
        }
    }
}

/// Broken-circle spinner frames, shared between the TUI render path and the keyed
/// controller's `most_recent()` result.
const SPINNER_FRAMES: &[&str] = &["◜", "◠", "◝", "◞", "◡", "◟"];

/// Return the arc spinner frame appropriate for the given wall-clock
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
    /// Delivery audience for this status. Defaults to [`StatusScope::All`]; the
    /// web actor sets it from the originating connection so per-connection
    /// filtering can suppress other clients' operation toasts.
    pub scope: StatusScope,
    /// Whether this status waits for the user instead of leaving on its own.
    /// See the field of the same name on [`KeyedWireStatus`].
    pub sticky: bool,
    pub generation: Generation,
    /// Wall-clock time when this status was last set. Used for auto-clear and
    /// busy-timeout decisions in `tick`.
    since: Instant,
    /// When this entry was last heard from: either its `set`, or the most recent
    /// liveness heartbeat from [`tick`](KeyedStatusController::tick).
    ///
    /// Deliberately SEPARATE from `since`, which orders the entries for
    /// `most_recent()` and drives the spinner animation. The busy timeout asks
    /// "how long has this operation been silent", not "how old is this message",
    /// and folding the two would let a heartbeat on a background operation steal
    /// the TUI's single status line from a status the user just triggered.
    heartbeat: Instant,
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
    /// Delivery audience, carried so the on-connect status snapshot can be
    /// filtered per connection (a mid-operation joiner must not receive another
    /// connection's in-progress `Busy`). Defaults to [`StatusScope::All`].
    pub scope: StatusScope,
    /// Whether the surface must keep this message up until the user dismisses
    /// it, rather than retiring it on a timer.
    ///
    /// Deliberately orthogonal to tone: a catastrophic error is still visually an
    /// error, so this answers one crisp question instead, does the message wait
    /// for the user or leave on its own.
    ///
    /// The rule for setting it, and it is meant to stay rare: the user must act
    /// outside the toast to recover, or something may have been lost or left
    /// half-done. Everything else self-dismisses.
    pub sticky: bool,
}

/// What `tick` changed, so the web actor can broadcast precise StatusCleared /
/// status frames and the TUI can re-render.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusTickChanges {
    pub cleared_keys: Vec<Option<String>>, // None = the anonymous slot cleared
    pub upgraded: Vec<KeyedWireStatus>,    // busy→warning replacements
    /// Busy entries whose operation is still registered as running, re-stamped
    /// instead of upgraded. The caller must re-broadcast each one as a live
    /// `busy` status.
    ///
    /// The re-broadcast is required: a browser holds its own leak guard on every
    /// spinner (`BUSY_TOAST_MAX_MS` in `crates/dux-web/web/src/lib/notify.ts`),
    /// and only another frame on the same key re-arms it.
    pub refreshed: Vec<KeyedWireStatus>,
    /// How many finals aged out of the replay window under
    /// [`StatusRetention::Emit`]. Deliberately a count and not a key list: the
    /// caller must refresh its published snapshot but send no frame for them,
    /// because a `status_cleared` would dismiss the toast on every screen showing
    /// it, `sticky` ones included.
    pub purged: usize,
}

/// What the controller does with a final status (anything that is not
/// [`StatusTone::Busy`]: info/success, warning, error).
///
/// A `Busy` is live state while a final is an event. A surface that can be
/// joined late (the web, where every page load and every reconnect replays the
/// snapshot) must be told about work still in flight, and must not still be
/// told, an hour on, about an outcome that is long over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusRetention {
    /// Store a final and keep it until its turn on the line is over. The TUI's
    /// single status line has no other way to show an outcome, and it is never
    /// "reconnected", so a message stays on screen rather than being broadcast
    /// and forgotten. Because there is only one line, the entries queue under
    /// this policy instead of overwriting each other; the rules are on
    /// [`KeyedStatusController`].
    Retain,
    /// Treat a final as an event with a short tail: it is broadcast live by the
    /// caller (the web emitter sends the `WireStatus` it was given), kept
    /// replayable for [`FINAL_REPLAY_WINDOW`], and then dropped by
    /// [`tick`](KeyedStatusController::tick).
    ///
    /// The window is the point: without it, a socket that drops while an
    /// operation is running comes back to a snapshot holding nothing, because the
    /// busy was retired by its own final and that final was broadcast to nobody.
    ///
    /// The expiry is silent: no cleared key is reported, because the on-screen
    /// lifetime belongs to the client, which retires each toast on its own timer
    /// and never retires a `sticky` one.
    Emit,
}

/// A keyed multi-status controller.
///
/// Holds one anonymous slot (for unkeyed transient messages) and a
/// `String → KeyedStatus` map for named operations. Each emit bumps a
/// generation token on its key so that a stale-success clear from a prior
/// attempt can never silently dismiss a newer, live status.
///
/// The two retention policies render differently. Under
/// [`StatusRetention::Emit`] the web stacks every open status as its own toast
/// and dismisses them independently. Under [`StatusRetention::Retain`] the TUI
/// has one line, so the entries form a short queue: infos take the line in
/// arrival order, each for its full `ui.status_clear_seconds` window from the
/// moment it is shown; a warning or an error pre-empts and drops the infos still
/// waiting behind it; a busy takes the line at once and drops nothing. See
/// [`Self::store_queued`] and [`Self::advance`].
pub struct KeyedStatusController {
    /// The anonymous slot; most-recent-wins. Written under
    /// [`StatusRetention::Emit`] only: the TUI's queue needs a per-message
    /// identity, so under `Retain` an unkeyed message goes into
    /// [`Self::entries`] under a synthetic [`anon_storage_key`] instead.
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
    /// What happens to a final (non-`Busy`) status. See [`StatusRetention`].
    retention: StatusRetention,
    /// Which keys still have an operation running behind them. The controller
    /// reads it to decide whether a timed-out busy is stranded or merely slow,
    /// and retires a key whenever a final lands on it, which is the one place
    /// every final of every origin passes through.
    ///
    /// Default-empty, so a controller nobody handed a shared set to falls back
    /// to timing a busy out after [`BUSY_TIMEOUT`].
    live: LiveStatusKeys,
    /// The TUI's status queue: storage keys of [`Self::entries`] in the order
    /// they take the single line, front first. Populated only under
    /// [`StatusRetention::Retain`]; the web stacks every open status as a toast
    /// and has nothing to queue, so under `Emit` this stays empty and every
    /// queue-aware path is skipped.
    queue: VecDeque<String>,
    /// Which entry is on the line and when it went there.
    ///
    /// The instant gives an `Info` its full window from the moment it is shown
    /// rather than from the moment it was posted, so a message that waited its
    /// turn still gets read. The key travels with it so [`Self::advance`] can
    /// notice the front changed underneath it and re-stamp, rather than relying
    /// on every mutation remembering to reset a bare instant.
    shown: Option<(String, Instant)>,
    /// Storage key of the newest anonymous entry under `Retain`, so [`Self::pin`],
    /// [`Self::anon_generation`] and [`Self::anon_busy_matches`] can still name
    /// "the unkeyed message" now that there may be several queued at once.
    anon_key: Option<String>,
    /// Monotonic counter behind [`anon_storage_key`].
    next_anon: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusTickAction {
    Keep,
    Purge,
    Upgrade,
    /// The entry has been busy past [`BUSY_LIVE_CEILING`], so it is upgraded
    /// whatever liveness claims.
    Stalled,
    /// The busy timeout came due but the operation behind the key is still
    /// registered as running, so the entry is re-stamped and re-broadcast.
    Heartbeat,
}

/// Why a `Busy` is being replaced by a warning, which decides what the warning
/// says. The two cases are genuinely different facts and must not share wording:
/// one is silence from an operation nobody is waiting on, the other is an
/// operation dux IS still waiting on that has said nothing for half an hour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BusyUpgrade {
    TimedOut,
    Stalled,
}

impl BusyUpgrade {
    fn message(self) -> String {
        match self {
            BusyUpgrade::TimedOut => "timed out, check dux.log".to_string(),
            BusyUpgrade::Stalled => format!(
                "This operation has reported nothing for {} minutes, so dux has stopped showing it as running. It may still be going; check dux.log.",
                BUSY_LIVE_CEILING.as_secs() / 60
            ),
        }
    }

    fn log_word(self) -> &'static str {
        match self {
            BusyUpgrade::TimedOut => "timed-out",
            BusyUpgrade::Stalled => "stalled",
        }
    }
}

#[derive(Default)]
struct KeyedTickActions {
    purge: Vec<String>,
    upgrade: Vec<String>,
    stalled: Vec<String>,
    heartbeat: Vec<String>,
}

impl KeyedStatusController {
    /// A controller that RETAINS finals: the historical behaviour, and what the
    /// TUI wants. Its single status line shows the last message until something
    /// replaces it.
    pub fn with_clear_after(clear_after: Duration) -> Self {
        Self::with_retention(clear_after, StatusRetention::Retain)
    }

    /// A controller that treats a final as an event with a short tail: broadcast
    /// live, replayable for [`FINAL_REPLAY_WINDOW`], then dropped. This is what
    /// the web engine actor uses.
    ///
    /// It takes no `clear_after`, and that is deliberate rather than an
    /// oversight: `ui.status_clear_seconds` sets how long a toast stays ON
    /// SCREEN, which under this policy is the client's business, and the only
    /// server-side lifetime left is the replay window, which is a fixed constant
    /// for the reasons on [`FINAL_REPLAY_WINDOW`]. Passing a setting that could
    /// not affect anything would have been a lie in the signature.
    pub fn emitting_finals() -> Self {
        Self::with_retention(Duration::ZERO, StatusRetention::Emit)
    }

    /// `clear_after` is meaningful only for [`StatusRetention::Retain`]; the
    /// `Emit` path never reads it (see [`Self::emitting_finals`]).
    fn with_retention(clear_after: Duration, retention: StatusRetention) -> Self {
        Self {
            anon: None,
            entries: IndexMap::new(),
            clear_after,
            next_seq: 0,
            next_gen: 0,
            anon_pinned: false,
            retention,
            live: LiveStatusKeys::default(),
            queue: VecDeque::new(),
            shown: None,
            anon_key: None,
            next_anon: 0,
        }
    }

    /// Share the engine's [`LiveStatusKeys`] with this controller (builder
    /// form). Every surface that renders engine statuses must call it: without
    /// it the controller is back to guessing that a silent operation is a
    /// stranded one.
    pub fn with_live_keys(mut self, live: LiveStatusKeys) -> Self {
        self.live = live;
        self
    }

    /// Exempt the CURRENT anonymous-slot message from auto-clear. Used for the
    /// TUI's first-run help hint so it persists until the user's first action
    /// replaces it. A subsequent anonymous `set` clears the pin.
    pub fn pin(&mut self) {
        self.anon_pinned = true;
    }

    /// The generation of the NEWEST unkeyed message, or `None` when there is
    /// none.
    ///
    /// The unkeyed producers all share one identity here and none of them can
    /// tell the others apart, so "is my message still the one on the line" cannot be
    /// answered by tone (several producers write warnings) or by text
    /// (comparing strings would match a second producer's identical message).
    /// A producer that wants to retire its own message keeps the generation its
    /// `set` returned and clears only while this still equals it.
    pub fn anon_generation(&self) -> Option<Generation> {
        self.newest_anonymous().map(|a| a.generation)
    }

    /// The newest unkeyed entry, wherever this controller keeps it: the single
    /// anonymous slot under `Emit`, or the newest queued unkeyed entry under
    /// `Retain`.
    fn newest_anonymous(&self) -> Option<&KeyedStatus> {
        if self.retention == StatusRetention::Retain {
            return self.entries.get(self.anon_key.as_deref()?);
        }
        self.anon.as_ref()
    }

    pub fn set_clear_after(&mut self, clear_after: Duration) {
        self.clear_after = clear_after;
    }

    /// Set/replace a status.
    ///
    /// - `key == None` writes an unkeyed message: the anonymous slot under
    ///   `Emit` (most-recent-wins), a queue position of its own under `Retain`.
    ///   An EMPTY unkeyed message is the TUI's "clear the line" gesture and
    ///   takes the unkeyed entries off the queue instead of joining it.
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
        self.set_scoped(now, key, tone, message, StatusScope::All, false)
    }

    /// Like [`set`](Self::set) but records the status's delivery [`StatusScope`].
    /// `set` delegates here with [`StatusScope::All`], so TUI call sites (which
    /// ignore scope) need no change; the web actor calls this with the
    /// originating connection's scope so the snapshot can be filtered, and its
    /// `sticky` flag so a status that must wait for the user survives the
    /// surface's own auto-dismiss timer.
    pub fn set_scoped(
        &mut self,
        now: Instant,
        key: Option<String>,
        tone: StatusTone,
        message: impl Into<String>,
        scope: StatusScope,
        sticky: bool,
    ) -> Generation {
        let generation = Generation(self.next_gen);
        let seq = self.next_seq;
        self.next_gen += 1;
        self.next_seq += 1;

        let entry = KeyedStatus {
            key: key.clone(),
            tone,
            message: message.into(),
            scope,
            sticky,
            generation,
            since: now,
            heartbeat: now,
            seq,
        };

        // Under `Retain` every entry, unkeyed ones included, lives in `entries`
        // under a storage key so it can hold a position in the queue.
        if self.retention == StatusRetention::Retain {
            self.store_queued(now, key, entry);
            return generation;
        }

        // Both policies store the entry, a final included; `Emit` differs only in
        // how long it keeps one. Storing is what retires the `Busy` it replaces.
        match key {
            None => {
                self.anon = Some(entry);
                // A new anonymous set always clears the pin so the new message
                // follows normal auto-clear rules (the pin was for the old one).
                self.anon_pinned = false;
            }
            Some(k) => {
                // A final is the one moment every finished operation passes
                // through, so liveness is retired here; a `Busy` leaves the
                // registration alone, because `progress` re-emits one mid-run.
                if tone != StatusTone::Busy {
                    self.live.retire(&k);
                }
                self.entries.insert(k, entry);
            }
        }

        generation
    }

    /// Store an entry under [`StatusRetention::Retain`] and give it a place in
    /// the TUI's queue.
    ///
    /// The tone rules the single line follows live here:
    /// - an `Info` joins the back of the queue and waits its turn;
    /// - a `Warning` or an `Error` pre-empts, taking the line at once and
    ///   dropping every `Info` behind it, which has been overtaken and is stale;
    /// - a `Busy` also takes the line at once but drops nothing, because it is
    ///   live state rather than an outcome: the waiting infos resume after its
    ///   final.
    ///
    /// An `Error` on the line is retired by the arrival of any newer message,
    /// whatever its tone (see [`Self::retire_showing_error`]), which is what
    /// "until it is replaced" means on a queued line.
    ///
    /// A replacement on a key already in the queue keeps its position, whatever
    /// its tone: the queue orders news, and a producer restating itself on the
    /// key it already holds has produced none.
    fn store_queued(&mut self, now: Instant, key: Option<String>, entry: KeyedStatus) {
        let tone = entry.tone;
        let storage = match key {
            Some(k) => {
                if tone != StatusTone::Busy {
                    self.live.retire(&k);
                }
                k
            }
            None => {
                // The pin always belonged to the message that was last set.
                self.anon_pinned = false;
                match self.anon_storage_for(&entry.message) {
                    Some(k) => k,
                    None => {
                        self.drop_newest_anonymous();
                        return;
                    }
                }
            }
        };

        // Some entries wait for a REPLACEMENT rather than for a clock, and on a
        // queued line the thing that replaces them is whatever is said next.
        self.retire_replaced_by_arrival(&storage);

        let queued = self.queue.contains(&storage);
        self.entries.insert(storage.clone(), entry);

        // Pre-emption applies whether or not the entry is new to the queue, so a
        // busy whose final is a warning still clears the stale news behind it.
        if matches!(tone, StatusTone::Warning | StatusTone::Error) {
            self.drop_waiting_infos(&storage);
        }
        if !queued {
            if tone == StatusTone::Info {
                self.queue.push_back(storage.clone());
            } else {
                self.queue.push_front(storage.clone());
            }
            self.enforce_queue_bound();
        }
        if self.queue.front() == Some(&storage) {
            // Stamped here rather than at the next tick: a message is on the line
            // from the instant it is set, and a replacement in place restarts the
            // window rather than inheriting the old one's.
            self.shown = Some((storage, now));
        }
    }

    /// The storage key an anonymous message should be written to, or `None` when
    /// the message is the empty "clear the line" one every TUI producer uses to
    /// say it has nothing left to report.
    ///
    /// A repeat of a message already in the queue reuses that entry's key rather
    /// than minting a new one: producers that restate a standing condition on
    /// every selection move (the missing-project warning) would otherwise fill
    /// the queue with copies of one sentence, and a second copy of a message is
    /// not a second thing to read.
    fn anon_storage_for(&mut self, message: &str) -> Option<String> {
        if message.is_empty() {
            return None;
        }
        let existing = self
            .queue
            .iter()
            .find(|k| {
                self.entries
                    .get(*k)
                    .is_some_and(|e| e.key.is_none() && e.message == message)
            })
            .cloned();
        let key = match existing {
            Some(k) => k,
            None => {
                let k = anon_storage_key(self.next_anon);
                self.next_anon += 1;
                k
            }
        };
        self.anon_key = Some(key.clone());
        Some(key)
    }

    /// Retire the entry on the line that this arrival replaces.
    ///
    /// Two entries wait to be replaced rather than for a clock, and on a queued
    /// line nothing would ever replace them unless the arrival itself did:
    ///
    /// - An `Error`, always. It has no dwell clock, so on a queue an unkeyed
    ///   error would hold the line for the rest of the session with everything
    ///   behind it unreadable.
    /// - Every tone but `Busy` when `clear_after` is zero. That setting means
    ///   "never auto-clear", so there is no window to wait out and the line is
    ///   most-recent-wins.
    ///
    /// A `Busy` is the deliberate exception at a zero window: a spinner is live
    /// state, and what replaces it is its own final on its own key. An arrival
    /// queues behind it, and if that final never comes the busy timeout upgrades
    /// the spinner to a warning, which the next arrival may replace.
    ///
    /// A `sticky` entry and a pinned one are never retired here whatever the
    /// window: both flags mean the message waits for a person.
    fn retire_replaced_by_arrival(&mut self, incoming: &str) {
        let Some(front) = self.queue.front().cloned() else {
            return;
        };
        if front == incoming || (self.anon_pinned && self.anon_key.as_deref() == Some(&*front)) {
            return;
        }
        let Some(entry) = self.entries.get(&front) else {
            return;
        };
        let replaceable = !entry.sticky
            && match entry.tone {
                StatusTone::Busy => false,
                StatusTone::Error => true,
                StatusTone::Info | StatusTone::Warning => self.clear_after.is_zero(),
            };
        if replaceable {
            self.remove_queued(&front);
        }
    }

    /// Take the newest unkeyed entry off the queue: the TUI's empty-message set
    /// is one producer saying it has nothing left to report, which is a claim
    /// about its own message alone. A producer holding a generation should use
    /// [`Self::clear_anonymous_generation`], which names its message exactly.
    fn drop_newest_anonymous(&mut self) {
        let Some(key) = self.anon_key.clone() else {
            return;
        };
        self.remove_queued(&key);
    }

    /// Remove the unkeyed entry with this exact generation, wherever it is.
    ///
    /// The unkeyed producers share one identity and cannot tell each other's
    /// messages apart by tone or by text, so a producer that must retire its own
    /// message keeps the generation its [`set`](Self::set) returned and names it
    /// here. The blunt alternative, clearing the unkeyed line, takes somebody
    /// else's standing warning with it.
    ///
    /// Returns `true` when something was removed.
    pub fn clear_anonymous_generation(&mut self, generation: Generation) -> bool {
        if self.retention == StatusRetention::Retain {
            let doomed = self
                .queue
                .iter()
                .find(|k| {
                    self.entries
                        .get(*k)
                        .is_some_and(|e| e.key.is_none() && e.generation == generation)
                })
                .cloned();
            let Some(doomed) = doomed else { return false };
            if self.anon_key.as_deref() == Some(&*doomed) {
                self.anon_pinned = false;
            }
            self.remove_queued(&doomed);
            return true;
        }
        if self
            .anon
            .as_ref()
            .is_some_and(|a| a.generation == generation)
        {
            self.anon = None;
            self.anon_pinned = false;
            return true;
        }
        false
    }

    /// Retire the newest open `Busy`, wherever it sits in the queue.
    ///
    /// A worker that ends with nothing to say has to take its own spinner down,
    /// and against a keyed busy an empty message is a no-op, so the spinner would
    /// sit there until the busy timeout called it a false "timed out".
    ///
    /// Deliberately not limited to the entry on the line: a spinner is pushed off
    /// the front by any warning or error arriving while the work runs, which is
    /// exactly the case where the operation ends quietly and nothing else takes
    /// its spinner down.
    ///
    /// This is the fallback, for paths that hold no key. A caller that knows its
    /// key must use [`Self::clear`] with it; the newest busy is a guess.
    ///
    /// Returns `true` when a spinner was taken down.
    pub fn retire_newest_busy(&mut self) -> bool {
        if self.retention == StatusRetention::Retain {
            let Some(newest) = self
                .queue
                .iter()
                .filter_map(|key| self.entries.get(key).map(|entry| (key, entry)))
                .filter(|(_, entry)| entry.tone == StatusTone::Busy)
                .max_by_key(|(_, entry)| entry.seq)
                .map(|(key, _)| key.clone())
            else {
                return false;
            };
            if let Some(key) = self.entries.get(&newest).and_then(|e| e.key.clone()) {
                self.live.retire(&key);
            }
            let was_showing = self.queue.front() == Some(&newest);
            self.remove_queued(&newest);
            if was_showing {
                self.shown = None;
            }
            return true;
        }
        if self
            .anon
            .as_ref()
            .is_some_and(|a| a.tone == StatusTone::Busy)
        {
            self.anon = None;
            return true;
        }
        false
    }

    /// Drop every `Info` in the queue except `except`, which is the pre-empting
    /// entry itself.
    fn drop_waiting_infos(&mut self, except: &str) {
        let doomed: Vec<String> = self
            .queue
            .iter()
            .filter(|k| {
                k.as_str() != except
                    && self
                        .entries
                        .get(*k)
                        .is_some_and(|e| e.tone == StatusTone::Info)
            })
            .cloned()
            .collect();
        for key in doomed {
            self.remove_queued(&key);
        }
    }

    /// Hold the queue to [`MAX_QUEUED_STATUSES`] waiters behind the line.
    fn enforce_queue_bound(&mut self) {
        while self.queue.len() > MAX_QUEUED_STATUSES + 1 {
            // Position 0 is on screen and is never taken out from under the
            // reader. Among the waiters the oldest `Info` goes first, and with
            // none left the oldest waiter of any tone.
            let Some(doomed) = self
                .oldest_waiter(true)
                .or_else(|| self.oldest_waiter(false))
            else {
                return;
            };
            self.remove_queued(&doomed);
        }
    }

    /// The waiter that has been queued longest, optionally restricted to
    /// `Info`s.
    ///
    /// Ordered by `seq`, the arrival counter, and never by queue position: the
    /// pre-empting tones are pushed to the front, so position 1 is the second
    /// newest of them, and evicting it would drop the freshest news while
    /// keeping a backlog of older warnings nobody can reach.
    fn oldest_waiter(&self, only_infos: bool) -> Option<String> {
        self.queue
            .iter()
            .skip(1)
            .filter_map(|key| self.entries.get(key).map(|entry| (key, entry)))
            .filter(|(_, entry)| !only_infos || entry.tone == StatusTone::Info)
            .min_by_key(|(_, entry)| entry.seq)
            .map(|(key, _)| key.clone())
    }

    /// Take one entry out of both the queue and the storage behind it.
    fn remove_queued(&mut self, key: &str) {
        self.queue.retain(|k| k != key);
        self.entries.shift_remove(key);
        if self.anon_key.as_deref() == Some(key) {
            self.anon_key = self.newest_anonymous_key();
        }
    }

    /// Whichever unkeyed entry is left with the highest arrival counter.
    ///
    /// Nulling [`Self::anon_key`] on a removal instead would tell the next
    /// producer that no unkeyed message exists while several are still queued.
    fn newest_anonymous_key(&self) -> Option<String> {
        self.queue
            .iter()
            .filter_map(|key| self.entries.get(key).map(|entry| (key, entry)))
            .filter(|(_, entry)| entry.key.is_none())
            .max_by_key(|(_, entry)| entry.seq)
            .map(|(key, _)| key.clone())
    }

    /// Move the TUI's queue on: stamp whatever is on the line, and retire it
    /// once its dwell is up so the next entry can be read.
    ///
    /// Pure in `now`. The only clock is the instant handed in, so neither a slow
    /// tick cadence nor a fast one can change how long a message is readable.
    fn advance(&mut self, now: Instant, changes: &mut StatusTickChanges) {
        loop {
            // A position whose entry has gone (a clear, an eviction) is not a
            // turn on the line.
            while self
                .queue
                .front()
                .is_some_and(|k| !self.entries.contains_key(k))
            {
                self.queue.pop_front();
            }
            let Some(front) = self.queue.front().cloned() else {
                self.shown = None;
                return;
            };
            let shown_at = match &self.shown {
                Some((key, at)) if *key == front => *at,
                _ => {
                    self.shown = Some((front.clone(), now));
                    now
                }
            };
            if !self.front_dwell_elapsed(&front, shown_at, now) {
                return;
            }
            self.queue.pop_front();
            if let Some(entry) = self.entries.shift_remove(&front) {
                changes.cleared_keys.push(entry.key.clone());
            }
            if self.anon_key.as_deref() == Some(front.as_str()) {
                self.anon_key = self.newest_anonymous_key();
            }
            self.shown = None;
        }
    }

    /// Whether the entry on the line has had its time.
    ///
    /// An `Info`'s window runs from when it was shown, so one that waited its
    /// turn is still readable for a full window. A `Warning`'s runs from when it
    /// was posted, so one that waited behind a newer warning shows only the
    /// retention it has left. A `Busy` and an `Error` have no dwell clock: a busy
    /// leaves when its final replaces it or the busy timeout upgrades it, and an
    /// error when the next message arrives, which
    /// [`Self::retire_showing_error`] handles.
    fn front_dwell_elapsed(&self, key: &str, shown_at: Instant, now: Instant) -> bool {
        let Some(entry) = self.entries.get(key) else {
            return true;
        };
        if entry.sticky {
            return false;
        }
        if self.anon_pinned && self.anon_key.as_deref() == Some(key) {
            return false;
        }
        if self.clear_after.is_zero() {
            // `status_clear_seconds = 0` means "never auto-clear", so there is no
            // window to wait out. An arrival retires whatever it replaces; this
            // covers what queued behind a `Busy`, which an arrival never touches.
            return entry.tone != StatusTone::Busy && self.queue.len() > 1;
        }
        match (entry.tone, entry.tone.clear_windows()) {
            (_, None) => false,
            (StatusTone::Info, Some(_)) => now.duration_since(shown_at) >= self.clear_after,
            (_, Some(windows)) => now.duration_since(entry.since) >= self.clear_after * windows,
        }
    }

    /// Remove a keyed entry only if the carried generation matches the stored one
    /// (the clear-race guard) and the entry is not [`sticky`].
    ///
    /// `generation == None` skips the generation check, but not the sticky one.
    ///
    /// A sticky entry is never removed by a clear, whichever form is used: a
    /// clear says the operation ended with nothing to say, while a sticky final
    /// says something is half-done and waiting for the user, and the sticky final
    /// is the newer, more specific fact. An operation that supersedes it must say
    /// so with a new [`set`](Self::set) on the key, which still replaces it.
    ///
    /// This strands nothing: under [`StatusRetention::Emit`] the replay window in
    /// [`tick`](Self::tick) still retires a sticky entry, silently, so the
    /// snapshot does not grow and the toast is left for the user to dismiss.
    ///
    /// Returns `true` if anything was removed, so a caller that broadcasts a
    /// dismissal only does it when one actually happened.
    ///
    /// [`sticky`]: KeyedWireStatus::sticky
    pub fn clear(&mut self, key: &str, generation: Option<Generation>) -> bool {
        // A clear is a final that had nothing to say, so it retires liveness
        // before the sticky and generation guards, which decide only what stays
        // on screen.
        self.live.retire(key);
        if let Some(entry) = self.entries.get(key) {
            if entry.sticky {
                return false;
            }
            let matches = match generation {
                None => true,
                Some(g) => entry.generation == g,
            };
            if matches {
                if self.retention == StatusRetention::Retain {
                    // The queue must let go of the position too, wherever the
                    // entry was in it: on the line, the line moves on.
                    self.remove_queued(key);
                } else {
                    self.entries.swap_remove(key);
                }
                return true;
            }
        }
        false
    }

    /// Expire timed-out entries.
    ///
    /// Under [`StatusRetention::Retain`]:
    /// - A final older than its tone's window ([`StatusTone::clear_windows`]:
    ///   one `clear_after` for `Info`, [`WARNING_CLEAR_FACTOR`] of them for
    ///   `Warning`, never for `Error`) is removed and reported in
    ///   `cleared_keys`. A `sticky` final and the pinned anonymous slot are
    ///   exempt, because both wait for the user rather than for a timer.
    ///
    /// Under [`StatusRetention::Emit`]:
    /// - Every final older than [`FINAL_REPLAY_WINDOW`] is removed silently,
    ///   with nothing reported: it is leaving the replay snapshot, not the
    ///   user's screen, and `clear_after` plays no part.
    ///
    /// Under both:
    /// - A `Busy` silent for longer than `busy_timeout` is upgraded in place to
    ///   a "timed out" `Warning`, restamping `since` so it gets a full replay
    ///   window, unless [`LiveStatusKeys`] says an operation is still registered
    ///   behind the key, in which case it is re-stamped and handed back in
    ///   `refreshed` for re-broadcast. A slow clone therefore keeps its spinner
    ///   for as long as it takes. Past [`BUSY_LIVE_CEILING`] the upgrade happens
    ///   whatever liveness says.
    ///
    /// Returns the set of changes the caller must broadcast.
    pub fn tick(&mut self, now: Instant, busy_timeout: Duration) -> StatusTickChanges {
        let mut changes = StatusTickChanges::default();
        self.tick_anonymous(now, busy_timeout, &mut changes);
        let actions = self.keyed_tick_actions(now, busy_timeout);
        let upgraded_keys: Vec<String> = actions
            .upgrade
            .iter()
            .chain(actions.stalled.iter())
            .cloned()
            .collect();
        self.purge_keyed_finals(actions.purge, &mut changes);
        self.upgrade_keyed_busys(actions.upgrade, now, &mut changes);
        self.stall_keyed_busys(actions.stalled, now, &mut changes);
        self.heartbeat_keyed_busys(actions.heartbeat, now, &mut changes);
        if self.retention == StatusRetention::Retain {
            // A busy upgraded into a warning is a warning taking the line, so the
            // infos behind it are stale; only when it is the front, though, since
            // one timing out down the queue says nothing about what is on screen.
            if let Some(front) = self.queue.front().cloned()
                && upgraded_keys.contains(&front)
            {
                self.drop_waiting_infos(&front);
            }
            // Last, so a busy the steps above upgraded into a warning is on the
            // line with its own fresh retention rather than being judged on the
            // instant the busy started.
            self.advance(now, &mut changes);
        }

        changes
    }

    fn tick_anonymous(
        &mut self,
        now: Instant,
        busy_timeout: Duration,
        changes: &mut StatusTickChanges,
    ) {
        if self.anonymous_final_expired(now) {
            self.anon = None;
            self.record_anonymous_expiry(changes);
        }
        if self.anonymous_busy_timed_out(now, busy_timeout) {
            self.upgrade_anonymous_busy(now, changes);
        }
    }

    fn anonymous_final_expired(&self, now: Instant) -> bool {
        let Some(entry) = self.anon.as_ref() else {
            return false;
        };
        if self.retention == StatusRetention::Emit {
            return entry.tone != StatusTone::Busy
                && now.duration_since(entry.since) >= FINAL_REPLAY_WINDOW;
        }
        !self.anon_pinned
            && !entry.sticky
            && !self.clear_after.is_zero()
            && entry.tone.clear_windows().is_some_and(|windows| {
                now.duration_since(entry.since) >= self.clear_after * windows
            })
    }

    fn record_anonymous_expiry(&self, changes: &mut StatusTickChanges) {
        if self.retention == StatusRetention::Emit {
            changes.purged += 1;
        } else {
            changes.cleared_keys.push(None);
        }
    }

    fn anonymous_busy_timed_out(&self, now: Instant, busy_timeout: Duration) -> bool {
        self.anon.as_ref().is_some_and(|entry| {
            !self.anon_pinned
                && entry.tone == StatusTone::Busy
                && now.duration_since(entry.since) >= busy_timeout
        })
    }

    fn upgrade_anonymous_busy(&mut self, now: Instant, changes: &mut StatusTickChanges) {
        let Some(anon) = self.anon.as_mut() else {
            return;
        };
        crate::logger::warn(&format!(
            "anonymous status left Busy with no final (\"{}\"); upgrading to a timed-out warning",
            anon.message
        ));
        anon.tone = StatusTone::Warning;
        anon.message = "timed out, check dux.log".to_string();
        anon.since = now;
        anon.heartbeat = now;
        anon.generation = Generation(self.next_gen);
        anon.seq = self.next_seq;
        self.next_gen += 1;
        self.next_seq += 1;
        changes.upgraded.push(KeyedWireStatus {
            key: None,
            tone: StatusTone::Warning.as_wire().to_string(),
            message: "timed out, check dux.log".to_string(),
            scope: anon.scope.clone(),
            sticky: false,
        });
    }

    fn keyed_tick_actions(&self, now: Instant, busy_timeout: Duration) -> KeyedTickActions {
        let mut actions = KeyedTickActions::default();
        for (key, entry) in &self.entries {
            match self.keyed_tick_action(key, entry, now, busy_timeout) {
                StatusTickAction::Keep => {}
                StatusTickAction::Purge => actions.purge.push(key.clone()),
                StatusTickAction::Upgrade => actions.upgrade.push(key.clone()),
                StatusTickAction::Stalled => actions.stalled.push(key.clone()),
                StatusTickAction::Heartbeat => actions.heartbeat.push(key.clone()),
            }
        }
        actions
    }

    fn keyed_tick_action(
        &self,
        key: &str,
        entry: &KeyedStatus,
        now: Instant,
        busy_timeout: Duration,
    ) -> StatusTickAction {
        // The pin exempts the unkeyed message it was taken on from every timer,
        // the busy timeout included, wherever this controller stores it.
        if self.anon_pinned && self.anon_key.as_deref() == Some(key) {
            return StatusTickAction::Keep;
        }
        let age = now.duration_since(entry.since);
        if entry.tone == StatusTone::Busy {
            // The ceiling is measured from `since` (when the operation started),
            // never from `heartbeat`, which liveness keeps moving forever.
            if age >= BUSY_LIVE_CEILING {
                return StatusTickAction::Stalled;
            }
            if now.duration_since(entry.heartbeat) < busy_timeout {
                return StatusTickAction::Keep;
            }
            return if self.live.is_live(key) {
                StatusTickAction::Heartbeat
            } else {
                StatusTickAction::Upgrade
            };
        }
        if self.retention == StatusRetention::Emit {
            return if age >= FINAL_REPLAY_WINDOW {
                StatusTickAction::Purge
            } else {
                StatusTickAction::Keep
            };
        }
        // Under `Retain` a final leaves through the queue in [`Self::advance`],
        // which is the only place that knows how long it has actually been on
        // the line rather than merely how long ago it was posted.
        StatusTickAction::Keep
    }

    fn purge_keyed_finals(&mut self, keys: Vec<String>, changes: &mut StatusTickChanges) {
        // Replay expiry is silent so it cannot dismiss a toast still shown by a client.
        for key in keys {
            self.entries.shift_remove(&key);
            changes.purged += 1;
        }
    }

    fn upgrade_keyed_busys(
        &mut self,
        keys: Vec<String>,
        now: Instant,
        changes: &mut StatusTickChanges,
    ) {
        for key in keys {
            if let Some(upgraded) = self.upgrade_keyed_busy(&key, now, BusyUpgrade::TimedOut) {
                changes.upgraded.push(upgraded);
            }
        }
    }

    /// Upgrade the busy entries that crossed [`BUSY_LIVE_CEILING`].
    ///
    /// Reaching here means liveness leaked: something registered a key and no
    /// final ever retired it. The message says what is actually known (nothing
    /// has been heard for that long) rather than claiming a timeout, and points
    /// at the log, because the leak itself is the thing worth reporting.
    fn stall_keyed_busys(
        &mut self,
        keys: Vec<String>,
        now: Instant,
        changes: &mut StatusTickChanges,
    ) {
        for key in keys {
            if let Some(upgraded) = self.upgrade_keyed_busy(&key, now, BusyUpgrade::Stalled) {
                changes.upgraded.push(upgraded);
            }
        }
    }

    /// Re-stamp the busy entries whose operation is still running and hand each
    /// one back for re-broadcast.
    ///
    /// `since` is deliberately left alone: the operation started when it started,
    /// and the spinner animation and the web's most-recent-wins ordering both
    /// read it. Only `heartbeat` moves, which is the field the timeout measures.
    fn heartbeat_keyed_busys(
        &mut self,
        keys: Vec<String>,
        now: Instant,
        changes: &mut StatusTickChanges,
    ) {
        for key in keys {
            let Some(entry) = self.entries.get_mut(&key) else {
                continue;
            };
            entry.heartbeat = now;
            changes.refreshed.push(KeyedWireStatus {
                key: entry.key.clone(),
                tone: entry.tone.as_wire().to_string(),
                message: entry.message.clone(),
                scope: entry.scope.clone(),
                sticky: entry.sticky,
            });
        }
    }

    fn upgrade_keyed_busy(
        &mut self,
        key: &str,
        now: Instant,
        reason: BusyUpgrade,
    ) -> Option<KeyedWireStatus> {
        // Whichever way a busy is upgraded, the operation behind it is no longer
        // being waited on, so its registration goes with it. Without this the
        // ceiling would fire again on every tick for the rest of the process.
        self.live.retire(key);
        let entry = self.entries.get_mut(key)?;
        crate::logger::warn(&format!(
            "status key \"{key}\" left Busy with no final (\"{}\"); upgrading to a {} warning",
            entry.message,
            reason.log_word()
        ));
        let generation = Generation(self.next_gen);
        let seq = self.next_seq;
        self.next_gen += 1;
        self.next_seq += 1;
        entry.tone = StatusTone::Warning;
        entry.message = reason.message();
        entry.sticky = false;
        entry.generation = generation;
        entry.since = now;
        entry.heartbeat = now;
        entry.seq = seq;
        Some(KeyedWireStatus {
            key: entry.key.clone(),
            tone: StatusTone::Warning.as_wire().to_string(),
            message: entry.message.clone(),
            scope: entry.scope.clone(),
            sticky: entry.sticky,
        })
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
                scope: anon.scope.clone(),
                sticky: anon.sticky,
            });
        }
        for entry in self.entries.values() {
            out.push(KeyedWireStatus {
                // `entry.key` and not the storage key: under `Retain` an unkeyed
                // entry is stored under a synthetic id it must not report.
                key: entry.key.clone(),
                tone: entry.tone.as_wire().to_string(),
                message: entry.message.clone(),
                scope: entry.scope.clone(),
                sticky: entry.sticky,
            });
        }
        out
    }

    /// The status on the line: the front of the queue under `Retain`, and the
    /// most-recently-set open status under `Emit`, where sequence numbers break
    /// ties between entries written at the same instant.
    fn most_recent_entry(&self) -> Option<&KeyedStatus> {
        // Under `Retain` the TUI's single line is a QUEUE, so what is on it is
        // the front of that queue, never simply the newest thing set.
        if self.retention == StatusRetention::Retain {
            return self.entries.get(self.queue.front()?);
        }
        let anon_ref = self.anon.as_ref();
        let keyed_ref = self.entries.values().max_by_key(|e| (e.since, e.seq));

        match (anon_ref, keyed_ref) {
            (None, None) => None,
            (Some(a), None) => Some(a),
            (None, Some(k)) => Some(k),
            (Some(a), Some(k)) => {
                if (a.since, a.seq) >= (k.since, k.seq) {
                    Some(a)
                } else {
                    Some(k)
                }
            }
        }
    }

    /// The one status on the line (keyed or anonymous), or `None` when nothing
    /// is open. Under `Retain` that is the front of the queue; under `Emit` it
    /// is the most-recently-set entry, and when two share the same `since`
    /// timestamp the one with the higher sequence number wins.
    pub fn most_recent(&self) -> Option<KeyedWireStatus> {
        let winner = self.most_recent_entry()?;

        Some(KeyedWireStatus {
            key: winner.key.clone(),
            tone: winner.tone.as_wire().to_string(),
            message: winner.message.clone(),
            scope: winner.scope.clone(),
            sticky: winner.sticky,
        })
    }

    /// Whether the anonymous (unkeyed) slot currently holds a `Busy` entry
    /// with the exact given message. Used by deletion workers to guard against
    /// clobbering a newer status that replaced their Busy while they ran.
    pub fn anon_busy_matches(&self, message: &str) -> bool {
        self.newest_anonymous()
            .is_some_and(|a| a.tone == StatusTone::Busy && a.message == message)
    }

    /// TUI projection: the status on the line as a `(tone, text)`
    /// pair suitable for direct rendering. For `Busy` entries the braille
    /// spinner is prepended exactly as [`StatusLine::text()`] does, using the
    /// entry's `since` instant so the animation stays wall-clock based.
    /// Returns `None` when no status is open.
    pub fn most_recent_tui(&self) -> Option<(StatusTone, String)> {
        let winner = self.most_recent_entry()?;

        let text = match winner.tone {
            StatusTone::Busy => {
                format!("{} {}", spinner_frame_for(winner.since), winner.message)
            }
            _ => winner.message.clone(),
        };
        Some((winner.tone, text))
    }

    // -----------------------------------------------------------------------
    // Single-status compatibility surface: thin wrappers over the most-recent
    // projection used by TUI tests and existing call sites.
    // -----------------------------------------------------------------------

    /// The tone of the status on the line, or `Info` when nothing
    /// is open (mirrors the previous `StatusLine::tone()` API).
    pub fn tone(&self) -> StatusTone {
        self.most_recent_tui()
            .map(|(t, _)| t)
            .unwrap_or(StatusTone::Info)
    }

    /// The rendered text of the status on the line (spinner
    /// prepended for `Busy`), or an empty string when nothing is open.
    /// Mirrors the previous `StatusLine::text()` API.
    pub fn text(&self) -> String {
        self.most_recent_tui().map(|(_, t)| t).unwrap_or_default()
    }

    /// The raw message of the status on the line without any
    /// spinner prefix, or an empty string when nothing is open. Mirrors the
    /// previous `StatusLine::message()` API.
    pub fn message(&self) -> String {
        self.most_recent_entry()
            .map(|winner| winner.message.clone())
            .unwrap_or_default()
    }

    /// Whether no status is currently open. Mirrors the previous
    /// `StatusLine::is_empty()` API.
    pub fn is_empty(&self) -> bool {
        self.anon.is_none() && self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BUSY_LIVE_CEILING, BUSY_TIMEOUT, FINAL_REPLAY_WINDOW, KeyedStatusController,
        LiveStatusKeys, MAX_QUEUED_STATUSES, StatusTone, WARNING_CLEAR_FACTOR,
    };
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
    fn anonymous_busy_expires_to_warning_after_timeout() {
        let t0 = Instant::now();
        let busy_timeout = Duration::from_secs(20);
        let mut c = KeyedStatusController::with_clear_after(Duration::from_secs(6));
        c.set(t0, None, StatusTone::Busy, "Loading…");
        // Before the bound: still Busy (anonymous slot never auto-expires Busy).
        let changes = c.tick(t0 + Duration::from_secs(19), busy_timeout);
        assert!(changes.upgraded.is_empty());
        assert_eq!(c.most_recent().unwrap().tone, "busy");
        // After the bound: upgraded in place to a timed-out Warning, broadcast.
        let changes = c.tick(t0 + Duration::from_secs(20), busy_timeout);
        assert_eq!(changes.upgraded.len(), 1);
        assert_eq!(changes.upgraded[0].key, None);
        assert_eq!(changes.upgraded[0].tone, "warning");
        let mr = c.most_recent().unwrap();
        assert_eq!(mr.tone, "warning");
        assert!(mr.message.to_lowercase().contains("timed out"));
    }

    #[test]
    fn tick_reports_retirements_and_keyed_upgrades_in_slot_order() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::from_secs(1));
        c.set(t0, None, StatusTone::Info, "anonymous final");
        c.set(t0, Some("clear-a".into()), StatusTone::Info, "final a");
        c.set(t0, Some("upgrade-a".into()), StatusTone::Busy, "busy a");
        c.set(t0, Some("clear-b".into()), StatusTone::Info, "final b");
        c.set(t0, Some("upgrade-b".into()), StatusTone::Busy, "busy b");

        let changes = c.tick(t0 + BUSY_TIMEOUT, BUSY_TIMEOUT);

        // Nothing is retired: the two busies took the line ahead of the finals
        // and their upgrades are warnings with fresh retention of their own, so
        // no queued entry has had its turn yet.
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        assert_eq!(
            changes
                .upgraded
                .iter()
                .map(|status| status.key.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("upgrade-a"), Some("upgrade-b")]
        );
        assert_eq!(c.entries["upgrade-a"].generation, super::Generation(5));
        assert_eq!(c.entries["upgrade-a"].seq, 5);
        assert_eq!(c.entries["upgrade-b"].generation, super::Generation(6));
        assert_eq!(c.entries["upgrade-b"].seq, 6);
        // The three infos behind the warnings are stale news and went with them.
        assert_eq!(c.snapshot().len(), 2);
    }

    #[test]
    fn tick_upgrades_anonymous_before_keyed_and_keeps_its_stored_sticky_flag() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::from_secs(1));
        c.set_scoped(
            t0,
            None,
            StatusTone::Busy,
            "anonymous busy",
            super::StatusScope::Connection("anon".into()),
            true,
        );
        c.set_scoped(
            t0,
            Some("keyed".into()),
            StatusTone::Busy,
            "keyed busy",
            super::StatusScope::Connection("keyed".into()),
            true,
        );

        let changes = c.tick(t0 + BUSY_TIMEOUT, BUSY_TIMEOUT);

        assert_eq!(
            changes
                .upgraded
                .iter()
                .map(|status| status.key.as_deref())
                .collect::<Vec<_>>(),
            vec![None, Some("keyed")]
        );
        assert!(changes.upgraded.iter().all(|status| !status.sticky));
        // Under `Retain` an unkeyed entry is stored the same way a keyed one is
        // so it can hold a queue position, so it is upgraded through the same
        // path and clears its sticky flag as the keyed one does. The old
        // anonymous slot kept the flag; that difference was an accident of
        // having two upgrade paths, not something any surface asked for.
        let anon = c
            .newest_anonymous()
            .cloned()
            .expect("the anonymous upgrade is still stored");
        assert_eq!(anon.generation, super::Generation(2));
        assert_eq!(anon.seq, 2);
        assert!(!anon.sticky);
        let keyed = &c.entries["keyed"];
        assert_eq!(keyed.generation, super::Generation(3));
        assert_eq!(keyed.seq, 3);
        assert!(!keyed.sticky, "the keyed stored entry clears its flag");
    }

    #[test]
    fn pinned_anonymous_busy_stays_busy_under_both_retention_policies() {
        let t0 = Instant::now();
        let retain = KeyedStatusController::with_clear_after(Duration::from_secs(1));
        let emit = KeyedStatusController::emitting_finals();

        for mut c in [retain, emit] {
            c.set(t0, None, StatusTone::Busy, "still running");
            c.pin();
            let changes = c.tick(t0 + Duration::from_secs(3600), BUSY_TIMEOUT);
            assert!(changes.upgraded.is_empty());
            assert_eq!(c.snapshot()[0].tone, "busy");
        }
    }

    #[test]
    fn a_warning_clears_after_three_windows_and_an_error_never_does() {
        let t0 = Instant::now();
        let window = Duration::from_secs(6);
        let mut c = KeyedStatusController::with_clear_after(window);
        // One tone at a time: the TUI's line is a queue now, so a warning and an
        // error set together do not race each other's clocks, they take turns.
        // Which one takes the line first is `the_newer_warning_shows_first...`.
        c.set(t0, None, StatusTone::Warning, "Already serving.");

        // One window in, a warning is still there: it outlives an info.
        let changes = c.tick(t0 + window, BUSY_TIMEOUT);
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        assert_eq!(c.snapshot().len(), 1);

        // A second short of three windows still keeps it.
        let changes = c.tick(t0 + window * 3 - Duration::from_secs(1), BUSY_TIMEOUT);
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        assert_eq!(c.snapshot().len(), 1);

        // At three windows it goes, announced.
        let changes = c.tick(t0 + window * 3, BUSY_TIMEOUT);
        assert_eq!(changes.cleared_keys, vec![None]);
        assert!(c.snapshot().is_empty());

        // An error takes the line and is still there an hour later.
        c.set(
            t0 + window * 3,
            Some("pull".into()),
            StatusTone::Error,
            "Pull failed.",
        );
        let _ = c.tick(t0 + Duration::from_secs(3600), BUSY_TIMEOUT);
        let snap = c.snapshot();
        assert_eq!(snap.len(), 1, "an error waits for a replacement: {snap:?}");
        assert_eq!(snap[0].key.as_deref(), Some("pull"));
    }

    #[test]
    fn a_zero_window_never_clears_on_a_timer() {
        // `status_clear_seconds = 0` means "never auto-clear", for every tone.
        // What takes a message off the line at a zero window is the next message
        // arriving, never a clock; see `a_zero_window_is_most_recent_wins_for_
        // every_tone_but_busy`.
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::ZERO);
        c.set(t0, None, StatusTone::Warning, "Already serving.");
        let changes = c.tick(t0 + Duration::from_secs(3600), BUSY_TIMEOUT);
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        assert_eq!(c.snapshot().len(), 1);

        c.set(t0, Some("save".into()), StatusTone::Info, "Saved.");
        let changes = c.tick(t0 + Duration::from_secs(7200), BUSY_TIMEOUT);
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        assert_eq!(c.snapshot().len(), 1);
    }

    #[test]
    fn a_sticky_warning_does_not_expire_under_retain() {
        // `sticky` means the status waits for the user, so the tone's window
        // does not apply to it on either slot.
        let t0 = Instant::now();
        let window = Duration::from_secs(6);
        let mut c = KeyedStatusController::with_clear_after(window);
        c.set_scoped(
            t0,
            None,
            StatusTone::Warning,
            "Saved the file but could not paste its path.",
            super::StatusScope::All,
            true,
        );
        c.set_scoped(
            t0,
            Some("upload".into()),
            StatusTone::Info,
            "Saved the file but could not paste its path.",
            super::StatusScope::All,
            true,
        );
        let changes = c.tick(t0 + Duration::from_secs(3600), BUSY_TIMEOUT);
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        assert_eq!(c.snapshot().len(), 2, "a sticky final waits for the user");
    }

    #[test]
    fn the_web_mirrors_the_warning_clear_factor() {
        // The status line and the browser toast must agree on how much longer a
        // warning lasts than an info.
        const NOTIFY_TS: &str = include_str!("../../dux-web/web/src/lib/notify.ts");
        let declared = NOTIFY_TS
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("export const WARNING_DURATION_FACTOR = ")
            })
            .expect("notify.ts must declare WARNING_DURATION_FACTOR");
        assert_eq!(
            // The declaration may or may not end in a semicolon depending on how
            // the file was last formatted; the number is what must match.
            declared.trim().trim_end_matches(';').trim(),
            super::WARNING_CLEAR_FACTOR.to_string(),
            "notify.ts WARNING_DURATION_FACTOR must match WARNING_CLEAR_FACTOR"
        );
    }

    #[test]
    fn the_anonymous_slot_reports_the_generation_of_the_message_on_it() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::from_secs(6));
        assert_eq!(c.anon_generation(), None, "an empty slot names nothing");

        let mine = c.set(t0, None, StatusTone::Warning, "Project path not found.");
        assert_eq!(c.anon_generation(), Some(mine));

        // Somebody else writes the slot. The first producer still holds `mine`,
        // and this is how it learns its own message is no longer there: without
        // it, a producer that clears "if the line holds a warning" wipes a
        // warning that belongs to someone else.
        let theirs = c.set(t0, None, StatusTone::Warning, "Restart to apply.");
        assert_ne!(mine, theirs);
        assert_eq!(c.anon_generation(), Some(theirs));

        // A keyed status is a different slot and leaves the answer alone.
        c.set(t0, Some("push".into()), StatusTone::Error, "Push failed.");
        assert_eq!(c.anon_generation(), Some(theirs));
    }

    #[test]
    fn keyed_info_auto_clears_anonymous_and_keyed() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::from_secs(6));
        c.set(t0, None, StatusTone::Info, "Saved.");
        c.set(t0, Some("commit".into()), StatusTone::Info, "Committed.");
        // Two infos in one burst are both read now: they take the line in turn,
        // each for its own full six-second window, rather than the second one
        // overwriting the first before anybody saw it.
        let changes = c.tick(t0 + Duration::from_secs(6), Duration::from_secs(20));
        assert_eq!(changes.cleared_keys, vec![None]);
        assert_eq!(c.message(), "Committed.");
        let changes = c.tick(t0 + Duration::from_secs(12), Duration::from_secs(20));
        assert_eq!(changes.cleared_keys, vec![Some("commit".to_string())]);
        assert!(c.is_empty());

        // A warning outlasts the info window: still on the line at seventeen
        // seconds and gone at eighteen, three times the six-second window.
        let t1 = t0 + Duration::from_secs(12);
        c.set(t1, Some("stale".into()), StatusTone::Warning, "Heads up.");
        let changes = c.tick(t1 + Duration::from_secs(17), Duration::from_secs(20));
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        let changes = c.tick(t1 + Duration::from_secs(18), Duration::from_secs(20));
        assert_eq!(changes.cleared_keys, vec![Some("stale".to_string())]);

        // An error outlasts everything.
        let t2 = t1 + Duration::from_secs(18);
        c.set(t2, Some("push".into()), StatusTone::Error, "Push error.");
        let _ = c.tick(t2 + Duration::from_secs(3600), Duration::from_secs(20));
        assert_eq!(c.snapshot().len(), 1);
        assert_eq!(c.snapshot()[0].key.as_deref(), Some("push"));
    }

    #[test]
    fn tui_keyed_clear_dismisses_the_line() {
        // Verifies the TUI most-recent-wins projection: a matching keyed clear
        // removes the entry so the TUI line becomes empty.
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
        // The spinner is one arc glyph followed by a space and the message.
        assert!(
            text.starts_with(['◜', '◠', '◝', '◞', '◡', '◟']),
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
        // The message that released the pin was queued behind it rather than
        // overwriting it, so it takes the line next and gets its own window.
        assert_eq!(c.message(), "Saved.");
        let changes = c.tick(t0 + Duration::from_secs(3613), Duration::from_secs(20));
        assert_eq!(changes.cleared_keys, vec![None]);
        assert!(c.most_recent().is_none());
    }

    // -----------------------------------------------------------------------
    // Retention policy: Retain (TUI) vs Emit (web)
    // -----------------------------------------------------------------------

    #[test]
    fn retain_policy_keeps_a_final_until_something_replaces_it() {
        // The TUI's contract, pinned so the Emit work cannot quietly change it:
        // an error stays on the single status line indefinitely, well past the
        // window that governs the web. Only an error: a warning has a window of
        // its own (see `a_warning_clears_after_three_windows_and_an_error_never_does`).
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::from_secs(6));
        c.set(t0, Some("push".into()), StatusTone::Error, "Push failed.");
        let _ = c.tick(t0 + Duration::from_secs(3600), Duration::from_secs(20));
        assert_eq!(
            c.snapshot().len(),
            1,
            "Retain must keep the error available"
        );
        assert_eq!(c.most_recent().unwrap().tone, "error");
    }

    #[test]
    fn emit_policy_replays_a_recent_final_so_a_reconnect_learns_the_outcome() {
        // The dropped-socket journey: the operation finishes while nobody is
        // listening, and the tab that comes back a few seconds later must be
        // handed the outcome. Without this it reconnects to an empty snapshot
        // and sits on a spinner that nothing will ever stop.
        let t0 = Instant::now();
        let mut c = KeyedStatusController::emitting_finals();
        c.set(t0, Some("del".into()), StatusTone::Busy, "Removing\u{2026}");
        c.set(
            t0 + Duration::from_secs(2),
            Some("del".into()),
            StatusTone::Error,
            "Worktree delete failed.",
        );
        // Five seconds in: comfortably inside the browser's reconnect budget.
        let _ = c.tick(t0 + Duration::from_secs(5), Duration::from_secs(20));
        let snap = c.snapshot();
        assert_eq!(
            snap.len(),
            1,
            "the final must still be replayable: {snap:?}"
        );
        assert_eq!(snap[0].tone, "error");
        assert_eq!(
            snap[0].key.as_deref(),
            Some("del"),
            "and it must have replaced the busy on its own key, not sit beside it"
        );
    }

    #[test]
    fn a_busy_whose_operation_is_still_running_is_never_called_timed_out() {
        // The bug this pins: creating an agent over a slow network. The clone ran
        // for minutes, and twenty seconds in the controller replaced the honest
        // "Pulling latest changes…" spinner with "timed out", which then aged off
        // the screen on the warning window. The user watched the operation vanish
        // and then succeed.
        let t0 = Instant::now();
        // The clone is still running, so the engine still holds the create op.
        let live = LiveStatusKeys::default();
        live.register("create-1");
        let mut c = KeyedStatusController::emitting_finals().with_live_keys(live.clone());
        c.set(
            t0,
            Some("create-1".into()),
            StatusTone::Busy,
            "Pulling latest changes for project \"dux\" before creating the agent...",
        );

        let changes = c.tick(t0 + BUSY_TIMEOUT + Duration::from_secs(1), BUSY_TIMEOUT);
        assert!(
            changes.upgraded.is_empty(),
            "a running operation must not be reported as timed out, got {:?}",
            changes.upgraded
        );
        assert_eq!(
            changes.refreshed.len(),
            1,
            "and the surfaces must be told it is still going, got {:?}",
            changes.refreshed
        );
        assert_eq!(changes.refreshed[0].tone, "busy");
        assert_eq!(
            changes.refreshed[0].message,
            "Pulling latest changes for project \"dux\" before creating the agent..."
        );
        let snap = c.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].tone, "busy", "the spinner stays: {snap:?}");

        // Minutes later it is still going, and still a spinner.
        let mut now = t0 + BUSY_TIMEOUT + Duration::from_secs(1);
        for _ in 0..12 {
            now += BUSY_TIMEOUT;
            let _ = c.tick(now, BUSY_TIMEOUT);
        }
        assert_eq!(
            c.snapshot()[0].tone,
            "busy",
            "a five-minute clone keeps its spinner for the whole five minutes"
        );

        // The op finishes and its final replaces the spinner, as always.
        c.set(
            now,
            Some("create-1".into()),
            StatusTone::Info,
            "Agent created.",
        );
        assert_eq!(c.snapshot()[0].tone, "info");
        assert!(
            !live.is_live("create-1"),
            "and the final retired the registration, whatever produced it"
        );
    }

    #[test]
    fn a_final_of_any_origin_retires_liveness_through_the_controller() {
        // The registration is made by the engine at the spawn site, but it is
        // retired HERE, at the one door every finished operation passes through.
        // That is what makes liveness general instead of per registry: a plain
        // keyed `set`, a resolved op's message, and a bare clear all count.
        let t0 = Instant::now();
        for (name, finish) in [
            (
                "an info final",
                Box::new(|c: &mut KeyedStatusController| {
                    c.set(t0, Some("k".into()), StatusTone::Info, "done");
                }) as Box<dyn Fn(&mut KeyedStatusController)>,
            ),
            (
                "an error final",
                Box::new(|c: &mut KeyedStatusController| {
                    c.set(t0, Some("k".into()), StatusTone::Error, "boom");
                }),
            ),
            (
                "a clear",
                Box::new(|c: &mut KeyedStatusController| {
                    c.clear("k", None);
                }),
            ),
        ] {
            let live = LiveStatusKeys::default();
            live.register("k");
            let mut c = KeyedStatusController::emitting_finals().with_live_keys(live.clone());
            c.set(t0, Some("k".into()), StatusTone::Busy, "working");
            assert!(live.is_live("k"), "{name}: a busy leaves it registered");
            finish(&mut c);
            assert!(!live.is_live("k"), "{name} must retire the registration");
        }
    }

    #[test]
    fn a_progress_re_emit_leaves_the_operation_registered() {
        // A `HandlerStatusOp` re-emits a busy on its own key as the work moves
        // on ("Creating worktree…" then "Launching session…"). That is not a
        // final and must not retire anything.
        let t0 = Instant::now();
        let live = LiveStatusKeys::default();
        live.register("k");
        let mut c = KeyedStatusController::emitting_finals().with_live_keys(live.clone());
        c.set(t0, Some("k".into()), StatusTone::Busy, "Creating worktree…");
        c.set(t0, Some("k".into()), StatusTone::Busy, "Launching session…");
        assert!(live.is_live("k"));
    }

    #[test]
    fn a_leaked_registration_cannot_hold_a_spinner_forever() {
        // Belt and braces for the case liveness itself is wrong: something
        // registered a key and no final ever came. The spinner is not immortal;
        // past the ceiling it is replaced by a warning that says what is known
        // (nothing has been heard) rather than claiming a timeout, and names the
        // log.
        let t0 = Instant::now();
        let live = LiveStatusKeys::default();
        live.register("leaked");
        let mut c = KeyedStatusController::emitting_finals().with_live_keys(live.clone());
        c.set(t0, Some("leaked".into()), StatusTone::Busy, "Pulling…");

        // Just under the ceiling it is still a spinner, however many heartbeats
        // have gone by.
        let mut now = t0;
        while now.duration_since(t0) < BUSY_LIVE_CEILING - BUSY_TIMEOUT {
            now += BUSY_TIMEOUT;
            let _ = c.tick(now, BUSY_TIMEOUT);
        }
        assert_eq!(c.snapshot()[0].tone, "busy", "{:?}", c.snapshot());

        let changes = c.tick(t0 + BUSY_LIVE_CEILING, BUSY_TIMEOUT);
        assert_eq!(changes.upgraded.len(), 1, "got {:?}", changes.upgraded);
        assert_eq!(changes.upgraded[0].tone, "warning");
        assert!(
            changes.upgraded[0].message.contains("30 minutes"),
            "the warning must say how long it waited, got {:?}",
            changes.upgraded[0].message
        );
        assert!(
            changes.upgraded[0].message.contains("dux.log"),
            "and where to look, got {:?}",
            changes.upgraded[0].message
        );
        assert!(changes.refreshed.is_empty());
        assert!(
            !live.is_live("leaked"),
            "the leaked registration is dropped, or the ceiling fires every tick forever"
        );
    }

    #[test]
    fn a_busy_whose_operation_is_gone_still_times_out() {
        // The leak guard is why the timeout exists, and it must survive the fix:
        // an operation that vanished without a final still stops claiming work is
        // happening.
        let t0 = Instant::now();
        let mut c = KeyedStatusController::emitting_finals();
        c.set(
            t0,
            Some("create-1".into()),
            StatusTone::Busy,
            "Creating\u{2026}",
        );
        let changes = c.tick(t0 + BUSY_TIMEOUT, BUSY_TIMEOUT);
        assert_eq!(changes.upgraded.len(), 1, "got {:?}", changes.upgraded);
        assert_eq!(changes.upgraded[0].tone, "warning");
        assert_eq!(changes.upgraded[0].message, "timed out, check dux.log");
        assert!(changes.refreshed.is_empty());
    }

    #[test]
    fn a_heartbeat_never_steals_the_status_line_from_a_newer_message() {
        // `since` orders the TUI's single line. A heartbeat moves only the
        // timeout clock, so a background operation that has been running for
        // minutes cannot push aside the status the user just triggered.
        let t0 = Instant::now();
        let live = LiveStatusKeys::default();
        live.register("create-1");
        let mut c =
            KeyedStatusController::with_clear_after(Duration::from_secs(600)).with_live_keys(live);
        c.set(
            t0,
            Some("create-1".into()),
            StatusTone::Busy,
            "Creating\u{2026}",
        );
        c.set(
            t0 + BUSY_TIMEOUT,
            Some("push".into()),
            StatusTone::Busy,
            "Pushing\u{2026}",
        );

        let _ = c.tick(t0 + BUSY_TIMEOUT + Duration::from_secs(1), BUSY_TIMEOUT);

        let line = c.most_recent().expect("a status is open");
        assert_eq!(
            line.message, "Pushing\u{2026}",
            "the heartbeat must not reorder the line, got {line:?}"
        );
    }

    #[test]
    fn emit_policy_purges_every_tone_of_final_once_the_window_lapses() {
        // Replay expiry applies equally to info, warning, and error finals.
        let t0 = Instant::now();
        for tone in [StatusTone::Info, StatusTone::Warning, StatusTone::Error] {
            let mut c = KeyedStatusController::emitting_finals();
            c.set(t0, Some("k".into()), tone, "final");
            c.set(t0, None, tone, "unkeyed final");
            // One tick just before the boundary leaves both in place...
            let _ = c.tick(
                t0 + FINAL_REPLAY_WINDOW - Duration::from_millis(1),
                BUSY_TIMEOUT,
            );
            assert_eq!(
                c.snapshot().len(),
                2,
                "{tone:?} must survive to the boundary"
            );
            // ...and one at the boundary retires both, keyed and anonymous.
            let changes = c.tick(t0 + FINAL_REPLAY_WINDOW, BUSY_TIMEOUT);
            assert!(
                c.snapshot().is_empty(),
                "{tone:?} must stop being replayable, got {:?}",
                c.snapshot()
            );
            // SILENTLY. A cleared key becomes a `status_cleared` frame, which
            // would dismiss the toast on every screen showing it, including a
            // sticky one whose whole job is to wait for the user.
            assert!(
                changes.cleared_keys.is_empty(),
                "leaving the replay snapshot must not dismiss anyone's toast, got {:?}",
                changes.cleared_keys
            );
        }
    }

    #[test]
    fn emit_silently_purges_pinned_or_sticky_anonymous_finals() {
        let t0 = Instant::now();
        for (sticky, pinned) in [(true, false), (false, true)] {
            let mut c = KeyedStatusController::emitting_finals();
            c.set_scoped(
                t0,
                None,
                StatusTone::Warning,
                "final",
                super::StatusScope::All,
                sticky,
            );
            if pinned {
                c.pin();
            }

            let changes = c.tick(t0 + FINAL_REPLAY_WINDOW, BUSY_TIMEOUT);
            assert_eq!(changes.purged, 1);
            assert!(changes.cleared_keys.is_empty());
            assert!(c.snapshot().is_empty());
        }
    }

    #[test]
    fn emit_policy_never_purges_an_in_flight_busy() {
        // The window is for finals only. A `Busy` outlives it and is retired by
        // its own final or by the busy timeout, never by age alone.
        let t0 = Instant::now();
        let mut c = KeyedStatusController::emitting_finals();
        c.set(t0, Some("pull".into()), StatusTone::Busy, "Pulling\u{2026}");
        c.set(t0, None, StatusTone::Busy, "Loading\u{2026}");
        // A busy_timeout far past the replay window isolates the two rules.
        let long = FINAL_REPLAY_WINDOW * 10;
        let changes = c.tick(t0 + FINAL_REPLAY_WINDOW + Duration::from_secs(1), long);
        assert!(changes.upgraded.is_empty(), "not yet timed out");
        assert_eq!(
            c.snapshot().len(),
            2,
            "an operation still running must stay replayable however long it runs"
        );
    }

    #[test]
    fn emit_policy_final_dismisses_the_busy_it_replaces() {
        // The final still has to end the operation: a Busy left behind after its
        // final would be replayed as a spinner that never stops.
        let t0 = Instant::now();
        let mut c = KeyedStatusController::emitting_finals();
        c.set(t0, Some("pull".into()), StatusTone::Busy, "Pulling\u{2026}");
        c.set(t0, None, StatusTone::Busy, "Loading\u{2026}");
        c.set(t0, Some("pull".into()), StatusTone::Error, "Pull failed.");
        c.set(t0, None, StatusTone::Info, "Loaded.");
        let snap = c.snapshot();
        assert_eq!(snap.len(), 2, "one entry per slot, not two: {snap:?}");
        assert!(
            snap.iter().all(|e| e.tone != "busy"),
            "no spinner may survive its own final: {snap:?}"
        );
    }

    #[test]
    fn emit_policy_gives_a_stranded_busy_upgrade_its_own_window() {
        // The busy-timeout upgrade produces a Warning, which is a final, so it
        // is broadcast AND stays replayable for a full window measured from the
        // upgrade. A tab that dropped while the operation hung then reconnects
        // to the warning rather than to an empty snapshot.
        let t0 = Instant::now();
        let busy_timeout = Duration::from_secs(20);
        let mut c = KeyedStatusController::emitting_finals();
        c.set(
            t0,
            Some("launch".into()),
            StatusTone::Busy,
            "Launching\u{2026}",
        );
        c.set(t0, None, StatusTone::Busy, "Loading\u{2026}");
        let changes = c.tick(t0 + busy_timeout, busy_timeout);
        assert_eq!(
            changes.upgraded.len(),
            2,
            "both stranded busys must be reported as upgraded"
        );
        assert!(changes.upgraded.iter().all(|u| u.tone == "warning"));
        // Nothing may be reported as CLEARED: the client replaces the toast by
        // key from the upgraded broadcast, and a clear would dismiss the warning
        // the user is meant to read.
        assert!(
            changes.cleared_keys.is_empty(),
            "an upgrade is a replacement, not a dismissal"
        );
        assert_eq!(
            c.snapshot().len(),
            2,
            "the warnings must be replayable right after the upgrade"
        );
        // The window runs from the UPGRADE, not from when the busy started.
        let _ = c.tick(t0 + busy_timeout + FINAL_REPLAY_WINDOW, busy_timeout);
        assert!(
            c.snapshot().is_empty(),
            "and then they age out like any other final, got {:?}",
            c.snapshot()
        );
    }

    #[test]
    fn sticky_is_off_by_default_and_travels_into_the_snapshot() {
        // The flag is orthogonal to tone, so it has to be carried rather than
        // derived: an ordinary error is NOT sticky and only a status that asked
        // for it comes back sticky.
        use super::{StatusScope, StatusTone as T};
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::ZERO);
        c.set(t0, Some("ordinary".into()), T::Error, "Push failed.");
        // Read before the sticky one arrives: an arriving message retires the
        // ordinary error it replaces on the line, which is what `sticky` exempts
        // the other one from.
        let ordinary = c
            .snapshot()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("ordinary"))
            .expect("ordinary entry");
        c.set_scoped(
            t0,
            Some("halfdone".into()),
            T::Error,
            "Worktree delete failed.",
            StatusScope::All,
            true,
        );
        let snap = c.snapshot();
        let halfdone = snap
            .iter()
            .find(|e| e.key.as_deref() == Some("halfdone"))
            .expect("sticky entry");
        assert!(!ordinary.sticky, "the default must be non-sticky");
        assert!(halfdone.sticky, "a sticky status must stay sticky");
        assert!(
            c.most_recent().unwrap().sticky,
            "the single-line projection must carry the flag too"
        );
    }

    #[test]
    fn a_clear_can_never_dismiss_a_sticky_status() {
        // `sticky` means "this waits for the user". A server-side clear says
        // "the operation ended with nothing to say", which cannot be true of a
        // key that just reported something half-done: the sticky final is the
        // newer and more specific fact. If a clear could retire it, sticky would
        // be decorative for every engine-raised status, since a clear names only
        // a key and every keyed final is reachable by one.
        let t0 = Instant::now();
        let mut c = KeyedStatusController::emitting_finals();
        let g = c.set_scoped(
            t0,
            Some("del".into()),
            StatusTone::Error,
            "Worktree delete failed.",
            super::StatusScope::All,
            true,
        );
        // Neither the generation-matched clear nor the unconditional one may
        // touch it, and both must SAY they did nothing so the caller does not
        // broadcast a dismissal.
        assert!(!c.clear("del", Some(g)), "a matching clear must be refused");
        assert!(
            !c.clear("del", None),
            "an unconditional clear must be refused"
        );
        assert_eq!(c.snapshot().len(), 1, "the sticky status must survive");
        assert!(c.snapshot()[0].sticky);

        // The control: the same clear on a NON-sticky entry still works, so this
        // is a guard on stickiness and not a broken clear.
        let g2 = c.set(t0, Some("push".into()), StatusTone::Error, "Push failed.");
        assert!(c.clear("push", Some(g2)), "an ordinary final still clears");

        // And a sticky status is not immortal. A REPLACEMENT still replaces it,
        // because a later `set` carries new information for the user...
        c.set(
            t0,
            Some("del".into()),
            StatusTone::Info,
            "Cleaned up after all.",
        );
        assert_eq!(c.snapshot()[0].tone, "info");
        // ...and under Emit the replay window still retires it, so refusing the
        // clear cannot strand an entry in the snapshot forever.
        let mut c = KeyedStatusController::emitting_finals();
        c.set_scoped(
            t0,
            Some("del".into()),
            StatusTone::Error,
            "Worktree delete failed.",
            super::StatusScope::All,
            true,
        );
        let changes = c.tick(t0 + FINAL_REPLAY_WINDOW, BUSY_TIMEOUT);
        assert!(
            c.snapshot().is_empty(),
            "the window still purges a sticky entry"
        );
        assert!(
            changes.cleared_keys.is_empty(),
            "and does so silently, so the toast on screen is left alone"
        );
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

    // -----------------------------------------------------------------------
    // The TUI's status queue (StatusRetention::Retain)
    // -----------------------------------------------------------------------

    /// The TUI's configured auto-clear window in these tests.
    const WINDOW: Duration = Duration::from_secs(6);
    const TICK: Duration = Duration::from_millis(1);

    fn tui() -> KeyedStatusController {
        KeyedStatusController::with_clear_after(WINDOW)
    }

    /// What the TUI's single line is showing right now, without the spinner.
    fn line(c: &KeyedStatusController) -> Option<String> {
        c.most_recent().map(|s| s.message)
    }

    fn run(c: &mut KeyedStatusController, at: Instant) -> Option<String> {
        c.tick(at, BUSY_TIMEOUT);
        line(c)
    }

    #[test]
    fn queued_infos_are_shown_in_arrival_order_each_for_a_full_window() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Info, "first");
        c.set(t0, None, StatusTone::Info, "second");
        c.set(t0, None, StatusTone::Info, "third");

        assert_eq!(line(&c).as_deref(), Some("first"));
        assert_eq!(run(&mut c, t0 + WINDOW - TICK).as_deref(), Some("first"));
        // The second Info's own window starts when it is SHOWN, not when it was
        // posted, so it is still up a whole window later.
        assert_eq!(run(&mut c, t0 + WINDOW).as_deref(), Some("second"));
        assert_eq!(
            run(&mut c, t0 + WINDOW * 2 - TICK).as_deref(),
            Some("second")
        );
        assert_eq!(run(&mut c, t0 + WINDOW * 2).as_deref(), Some("third"));
        assert_eq!(run(&mut c, t0 + WINDOW * 3).as_deref(), None);
    }

    #[test]
    fn a_warning_pre_empts_and_drops_every_waiting_info() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Info, "info a");
        c.set(t0, None, StatusTone::Info, "info b");
        c.set(t0, None, StatusTone::Warning, "careful");

        assert_eq!(line(&c).as_deref(), Some("careful"));
        // A warning keeps its three windows, and nothing stale follows it.
        assert_eq!(
            run(&mut c, t0 + WINDOW * WARNING_CLEAR_FACTOR - TICK).as_deref(),
            Some("careful")
        );
        assert_eq!(
            run(&mut c, t0 + WINDOW * WARNING_CLEAR_FACTOR).as_deref(),
            None,
            "the infos behind the warning are stale news and are dropped"
        );
    }

    #[test]
    fn a_warning_pre_empts_but_lets_a_later_info_have_the_line_after_it() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Warning, "careful");
        c.set(t0, None, StatusTone::Info, "afterwards");

        assert_eq!(line(&c).as_deref(), Some("careful"));
        assert_eq!(
            run(&mut c, t0 + WINDOW * WARNING_CLEAR_FACTOR).as_deref(),
            Some("afterwards")
        );
        assert_eq!(
            run(&mut c, t0 + WINDOW * (WARNING_CLEAR_FACTOR + 1)).as_deref(),
            None
        );
    }

    #[test]
    fn the_newer_warning_shows_first_and_the_older_follows_with_what_is_left() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, Some("older".into()), StatusTone::Warning, "older");
        c.set(
            t0 + Duration::from_secs(1),
            Some("newer".into()),
            StatusTone::Warning,
            "newer",
        );
        assert_eq!(line(&c).as_deref(), Some("newer"));

        // The newer one leaves early, so the older one takes the line with the
        // retention it has left rather than a fresh three windows.
        assert!(c.clear("newer", None));
        assert_eq!(
            run(&mut c, t0 + Duration::from_secs(2)).as_deref(),
            Some("older")
        );
        assert_eq!(
            run(&mut c, t0 + WINDOW * WARNING_CLEAR_FACTOR - TICK).as_deref(),
            Some("older")
        );
        assert_eq!(
            run(&mut c, t0 + WINDOW * WARNING_CLEAR_FACTOR).as_deref(),
            None
        );
    }

    #[test]
    fn a_queued_warning_that_expired_while_waiting_never_reaches_the_line() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, Some("older".into()), StatusTone::Warning, "older");
        c.set(t0, Some("newer".into()), StatusTone::Error, "newer");
        assert_eq!(line(&c).as_deref(), Some("newer"));

        // An Error stays until it is replaced, so by the time it is cleared the
        // warning behind it is long past its own retention.
        assert!(c.clear("newer", None));
        assert_eq!(
            run(&mut c, t0 + WINDOW * WARNING_CLEAR_FACTOR).as_deref(),
            None
        );
    }

    #[test]
    fn a_busy_is_shown_at_once_drops_nothing_and_its_final_replaces_it_in_place() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, Some("op".into()), StatusTone::Busy, "working");
        c.set(t0, None, StatusTone::Info, "one");
        c.set(t0, None, StatusTone::Info, "two");
        assert_eq!(line(&c).as_deref(), Some("working"));

        // A busy never leaves on a dwell clock, however long the waiters wait
        // (and eighteen seconds is still inside the twenty-second busy timeout).
        assert_eq!(
            run(&mut c, t0 + WINDOW * WARNING_CLEAR_FACTOR).as_deref(),
            Some("working")
        );

        let done = t0 + WINDOW * WARNING_CLEAR_FACTOR;
        c.set(done, Some("op".into()), StatusTone::Info, "finished");
        assert_eq!(line(&c).as_deref(), Some("finished"));
        assert_eq!(run(&mut c, done + WINDOW).as_deref(), Some("one"));
        assert_eq!(run(&mut c, done + WINDOW * 2).as_deref(), Some("two"));
    }

    #[test]
    fn a_same_key_replacement_never_changes_queue_position() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Info, "showing");
        c.set(t0, Some("k".into()), StatusTone::Info, "waiting");
        c.set(t0, None, StatusTone::Info, "last");
        c.set(t0, Some("k".into()), StatusTone::Info, "waiting, revised");

        assert_eq!(line(&c).as_deref(), Some("showing"));
        assert_eq!(
            run(&mut c, t0 + WINDOW).as_deref(),
            Some("waiting, revised")
        );
        assert_eq!(run(&mut c, t0 + WINDOW * 2).as_deref(), Some("last"));
    }

    #[test]
    fn a_full_queue_drops_the_oldest_waiting_info_first() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Info, "showing");
        for i in 0..MAX_QUEUED_STATUSES {
            c.set(t0, None, StatusTone::Info, format!("q{i}"));
        }
        // One more than the queue holds: the oldest waiter goes, not the newest.
        c.set(t0, None, StatusTone::Info, "newest");

        let mut seen = vec![line(&c).expect("a line")];
        for step in 1..=MAX_QUEUED_STATUSES {
            seen.push(run(&mut c, t0 + WINDOW * step as u32).expect("a line"));
        }
        let mut expected = vec!["showing".to_string()];
        expected.extend((1..MAX_QUEUED_STATUSES).map(|i| format!("q{i}")));
        expected.push("newest".to_string());
        assert_eq!(seen, expected);
        assert_eq!(
            run(&mut c, t0 + WINDOW * (MAX_QUEUED_STATUSES as u32 + 1)),
            None
        );
    }

    #[test]
    fn clearing_a_key_removes_it_wherever_it_is_in_the_queue() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Info, "showing");
        c.set(t0, Some("k".into()), StatusTone::Info, "waiting");
        c.set(t0, None, StatusTone::Info, "after");
        assert!(c.clear("k", None));
        assert_eq!(line(&c).as_deref(), Some("showing"));
        assert_eq!(run(&mut c, t0 + WINDOW).as_deref(), Some("after"));

        // And clearing the one ON the line moves the line straight on.
        let mut c = tui();
        c.set(t0, Some("k".into()), StatusTone::Busy, "working");
        c.set(t0, None, StatusTone::Info, "behind");
        assert!(c.clear("k", None));
        assert_eq!(line(&c).as_deref(), Some("behind"));
    }

    #[test]
    fn the_dwell_clock_is_the_instant_handed_in_and_nothing_else() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Info, "first");
        c.set(t0, None, StatusTone::Info, "second");
        // Any number of ticks at the same instant never advance the queue.
        for _ in 0..50 {
            assert_eq!(run(&mut c, t0).as_deref(), Some("first"));
        }
        assert_eq!(run(&mut c, t0 + WINDOW).as_deref(), Some("second"));
    }

    #[test]
    fn an_empty_anonymous_message_clears_the_line_rather_than_queueing() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(
            t0,
            None,
            StatusTone::Warning,
            "Project path not found: /gone",
        );
        c.pin();
        c.set(t0, None, StatusTone::Info, "");
        assert_eq!(line(&c), None, "an empty anonymous message clears the line");
        assert!(c.is_empty());
    }

    #[test]
    fn a_repeated_anonymous_message_replaces_its_queued_copy() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Info, "showing");
        for _ in 0..20 {
            c.set(
                t0,
                None,
                StatusTone::Warning,
                "Project path not found: /gone",
            );
        }
        // The repeat is the same news, so it never stacks up behind itself.
        assert_eq!(line(&c).as_deref(), Some("Project path not found: /gone"));
        assert_eq!(
            run(&mut c, t0 + WINDOW * WARNING_CLEAR_FACTOR).as_deref(),
            None
        );
    }

    #[test]
    fn an_error_holds_the_line_only_until_the_next_message_arrives() {
        let t0 = Instant::now();

        // An unkeyed error is stored under an id nothing can ever write, so
        // without the arrival rule one `set_error` would freeze the line for the
        // session. Each tone in turn takes it away.
        let mut c = tui();
        c.set(t0, None, StatusTone::Error, "it went wrong");
        assert_eq!(
            run(&mut c, t0 + Duration::from_secs(3600)).as_deref(),
            Some("it went wrong"),
            "an error waits for a replacement rather than a clock"
        );
        c.set(t0, None, StatusTone::Info, "and then this happened");
        assert_eq!(line(&c).as_deref(), Some("and then this happened"));

        let mut c = tui();
        c.set(t0, None, StatusTone::Error, "it went wrong");
        c.set(t0, None, StatusTone::Warning, "careful");
        assert_eq!(line(&c).as_deref(), Some("careful"));

        let mut c = tui();
        c.set(t0, None, StatusTone::Error, "it went wrong");
        c.set(t0, Some("op".into()), StatusTone::Busy, "working");
        assert_eq!(line(&c).as_deref(), Some("working"));

        // A warning queued behind an error is reachable again.
        let mut c = tui();
        c.set(t0, Some("w".into()), StatusTone::Warning, "queued warning");
        c.set(t0, None, StatusTone::Error, "it went wrong");
        assert_eq!(line(&c).as_deref(), Some("it went wrong"));
        c.set(t0, Some("op".into()), StatusTone::Busy, "working");
        assert_eq!(line(&c).as_deref(), Some("working"));
        assert!(c.clear("op", None));
        assert_eq!(
            run(&mut c, t0).as_deref(),
            Some("queued warning"),
            "the warning the error was sitting on top of is readable again"
        );
    }

    #[test]
    fn every_info_after_an_error_is_read() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Error, "it went wrong");
        // Ten messages arriving over ten windows, the way a working session
        // produces them. Before the arrival rule the error swallowed all ten.
        let mut seen = Vec::new();
        for i in 0..10 {
            let at = t0 + WINDOW * i;
            c.set(at, None, StatusTone::Info, format!("step {i}"));
            c.tick(at, BUSY_TIMEOUT);
            seen.push(line(&c).expect("a line"));
        }
        let expected: Vec<String> = (0..10).map(|i| format!("step {i}")).collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn an_error_is_replaced_by_the_next_message_at_a_zero_window_too() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::ZERO);
        c.set(t0, None, StatusTone::Error, "it went wrong");
        c.set(t0, None, StatusTone::Info, "and then this happened");
        assert_eq!(line(&c).as_deref(), Some("and then this happened"));
        assert_eq!(
            run(&mut c, t0 + Duration::from_secs(3600)).as_deref(),
            Some("and then this happened"),
            "and nothing is left queued behind it"
        );
    }

    #[test]
    fn a_sticky_error_still_waits_for_the_user() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set_scoped(
            t0,
            None,
            StatusTone::Error,
            "half-done, and you must act",
            super::StatusScope::All,
            true,
        );
        c.set(t0, None, StatusTone::Info, "something else happened");
        assert_eq!(
            line(&c).as_deref(),
            Some("half-done, and you must act"),
            "sticky is the flag that says this one waits for a person"
        );
    }

    /// The other half of the same guard: a PINNED unkeyed error is not retired
    /// by an arrival either. The pin is how a producer says "this one is still
    /// true and still needs acting on", and it is the only thing standing
    /// between such a message and the next thing anybody says.
    #[test]
    fn a_pinned_error_is_not_retired_by_an_arrival() {
        let t0 = Instant::now();
        for window in [WINDOW, Duration::ZERO] {
            let mut c = KeyedStatusController::with_clear_after(window);
            c.set(t0, None, StatusTone::Error, "your config will not load");
            c.pin();
            c.set(t0, Some("other".into()), StatusTone::Info, "something else");
            assert_eq!(
                line(&c).as_deref(),
                Some("your config will not load"),
                "a pinned error waits for the user, at a {window:?} window"
            );
            assert_eq!(
                run(&mut c, t0 + Duration::from_secs(3600)).as_deref(),
                Some("your config will not load")
            );
        }
    }

    #[test]
    fn clearing_one_unkeyed_message_by_generation_leaves_the_others_alone() {
        let t0 = Instant::now();
        let mut c = tui();
        let theirs = c.set(t0, None, StatusTone::Warning, "Restart dux to apply.");
        c.pin();
        let mine = c.set(t0, None, StatusTone::Warning, "Project path not found.");

        // The second producer retires ITS OWN message and nothing else. Clearing
        // "the unkeyed line" would have taken the restart warning with it, which
        // is a message about something the user still has to do.
        assert!(c.clear_anonymous_generation(mine));
        assert_eq!(
            line(&c).as_deref(),
            Some("Restart dux to apply."),
            "the other producer's warning is still on the line"
        );
        assert_eq!(
            c.anon_generation(),
            Some(theirs),
            "and it is what the unkeyed slot names again"
        );
        assert!(
            !c.clear_anonymous_generation(mine),
            "a generation that is no longer there removes nothing"
        );
    }

    #[test]
    fn an_empty_unkeyed_message_retires_only_the_newest_unkeyed_entry() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Warning, "Restart dux to apply.");
        c.set(t0, None, StatusTone::Info, "Saved.");
        c.set(t0, None, StatusTone::Info, "");
        assert_eq!(
            line(&c).as_deref(),
            Some("Restart dux to apply."),
            "one producer's retraction is not a claim about every other one's"
        );
    }

    #[test]
    fn a_background_busy_timing_out_leaves_the_queue_behind_the_line_alone() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(
            t0,
            Some("background".into()),
            StatusTone::Busy,
            "background",
        );
        c.set(t0, None, StatusTone::Info, "still worth reading");
        // A newer busy takes the line, so the timing-out one is not on it.
        let later = t0 + BUSY_TIMEOUT - Duration::from_secs(1);
        c.set(
            later,
            Some("foreground".into()),
            StatusTone::Busy,
            "in front",
        );

        let changes = c.tick(t0 + BUSY_TIMEOUT, BUSY_TIMEOUT);
        assert_eq!(changes.upgraded.len(), 1, "{changes:?}");
        assert_eq!(line(&c).as_deref(), Some("in front"));
        assert!(c.clear("foreground", None));
        assert_eq!(
            run(&mut c, t0 + BUSY_TIMEOUT).as_deref(),
            Some("timed out, check dux.log"),
            "the upgraded warning takes the line it was queued for"
        );
        // And the info is still there: a background operation going quiet says
        // nothing about whether an unrelated message is still worth reading.
        assert!(c.clear("background", None));
        assert_eq!(
            run(&mut c, t0 + BUSY_TIMEOUT).as_deref(),
            Some("still worth reading")
        );
    }

    #[test]
    fn a_full_queue_of_warnings_drops_the_oldest_of_them() {
        let t0 = Instant::now();
        let mut c = tui();
        for i in 1..=8 {
            c.set(
                t0,
                Some(format!("w{i}")),
                StatusTone::Warning,
                format!("w{i}"),
            );
        }
        // Warnings pre-empt, so they stack newest-first: evicting by POSITION
        // would take the second newest and keep the oldest backlog.
        let held: Vec<String> = c.snapshot().into_iter().map(|s| s.message).collect();
        assert_eq!(held.len(), MAX_QUEUED_STATUSES + 1);
        assert!(!held.contains(&"w1".to_string()), "{held:?}");
        assert!(!held.contains(&"w2".to_string()), "{held:?}");
        for kept in 3..=8 {
            assert!(held.contains(&format!("w{kept}")), "{held:?}");
        }
    }

    #[test]
    fn a_busy_arriving_while_infos_wait_drops_none_of_them() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Info, "one");
        c.set(t0, None, StatusTone::Info, "two");
        c.set(t0, None, StatusTone::Info, "three");
        c.set(t0, Some("op".into()), StatusTone::Busy, "working");
        assert_eq!(line(&c).as_deref(), Some("working"));

        // The busy is live state, not an outcome: it says nothing about whether
        // what is queued behind it is still worth reading.
        assert!(c.clear("op", None));
        let mut seen = vec![run(&mut c, t0).expect("a line")];
        seen.push(run(&mut c, t0 + WINDOW).expect("a line"));
        seen.push(run(&mut c, t0 + WINDOW * 2).expect("a line"));
        assert_eq!(seen, vec!["one", "two", "three"]);
    }

    #[test]
    fn a_duplicate_set_never_moves_a_queued_key_to_the_back() {
        let t0 = Instant::now();
        let mut c = tui();
        c.set(t0, None, StatusTone::Info, "showing");
        c.set(t0, Some("middle".into()), StatusTone::Info, "middle");
        c.set(t0, None, StatusTone::Info, "last");
        // Re-setting the middle entry must not send it behind "last".
        c.set(t0, Some("middle".into()), StatusTone::Info, "middle");
        assert_eq!(run(&mut c, t0 + WINDOW).as_deref(), Some("middle"));
        assert_eq!(run(&mut c, t0 + WINDOW * 2).as_deref(), Some("last"));

        // And the same at the front, where a re-queue would put the busy behind
        // the infos it is meant to be running in front of.
        let mut c = tui();
        c.set(t0, Some("op".into()), StatusTone::Busy, "working");
        c.set(t0, None, StatusTone::Info, "waiting");
        c.set(t0, Some("op".into()), StatusTone::Busy, "working, still");
        assert_eq!(line(&c).as_deref(), Some("working, still"));
    }

    #[test]
    fn retiring_the_newest_busy_takes_a_keyed_spinner_down() {
        let t0 = Instant::now();
        let live = LiveStatusKeys::default();
        let mut c = tui().with_live_keys(live.clone());
        c.set(t0, None, StatusTone::Info, "queued behind it");
        live.register("op");
        c.set(t0, Some("op".into()), StatusTone::Busy, "working");

        assert!(c.retire_newest_busy());
        assert!(!live.is_live("op"), "the operation is over, so is its key");
        assert_eq!(line(&c).as_deref(), Some("queued behind it"));
        assert!(
            !c.retire_newest_busy(),
            "an info on the line is not a spinner to take down"
        );
    }

    /// The spinner that most needs taking down is exactly the one that is NOT on
    /// the line: work that ran long enough for a warning to arrive over it, and
    /// then ended with nothing to say. A front-only retirement leaves it for the
    /// busy timeout to call timed out, which it was not.
    #[test]
    fn a_spinner_pushed_off_the_line_by_a_warning_is_still_retired() {
        let t0 = Instant::now();
        let live = LiveStatusKeys::default();
        let mut c = tui().with_live_keys(live.clone());
        live.register("launch");
        c.set(
            t0,
            Some("launch".into()),
            StatusTone::Busy,
            "Launching\u{2026}",
        );
        c.set(t0, None, StatusTone::Warning, "something else went wrong");
        assert_eq!(line(&c).as_deref(), Some("something else went wrong"));

        assert!(c.retire_newest_busy(), "the spinner is behind the warning");
        assert!(!live.is_live("launch"));
        assert_eq!(
            line(&c).as_deref(),
            Some("something else went wrong"),
            "and the warning keeps the line it took"
        );

        // Nothing is left to time out into a false "timed out".
        let changes = c.tick(t0 + BUSY_TIMEOUT, BUSY_TIMEOUT);
        assert!(changes.upgraded.is_empty(), "{changes:?}");
    }

    #[test]
    fn a_zero_window_is_most_recent_wins_for_every_tone_but_busy() {
        let t0 = Instant::now();

        // Auto-clear off means no window to wait out, so queueing would freeze
        // the line for the life of the process. Every tone yields to the next
        // message instead, which is the "until the next one" the setting
        // promises. A warning was the one that froze: it has a window, so the
        // info rule did not cover it, and nothing else retired it either.
        for held in [StatusTone::Info, StatusTone::Warning, StatusTone::Error] {
            let mut c = KeyedStatusController::with_clear_after(Duration::ZERO);
            c.set(t0, None, held, "first");
            c.set(t0, None, StatusTone::Info, "second");
            assert_eq!(
                line(&c).as_deref(),
                Some("second"),
                "a {held:?} must not hold the line at a zero window"
            );
            assert_eq!(
                c.snapshot().len(),
                1,
                "and nothing is left queued: {:?}",
                c.snapshot()
            );
        }

        // Ten more messages after a warning are all read, none evicted unseen.
        let mut c = KeyedStatusController::with_clear_after(Duration::ZERO);
        c.set(t0, None, StatusTone::Warning, "Already serving.");
        for i in 0..10 {
            c.set(t0, None, StatusTone::Info, format!("step {i}"));
            assert_eq!(line(&c).as_deref(), Some(format!("step {i}").as_str()));
        }
    }

    /// The exception, and it is deliberate: a spinner is live state rather than
    /// an outcome, so at a zero window an arrival queues behind it and waits for
    /// the operation's own final. Whatever queued behind then takes the line as
    /// soon as that final lands, with no window to wait out.
    #[test]
    fn a_zero_window_still_lets_a_live_busy_keep_the_line_until_its_final() {
        let t0 = Instant::now();
        let live = LiveStatusKeys::default();
        let mut c =
            KeyedStatusController::with_clear_after(Duration::ZERO).with_live_keys(live.clone());
        live.register("op");
        c.set(t0, Some("op".into()), StatusTone::Busy, "working");
        c.set(t0, None, StatusTone::Info, "meanwhile");
        assert_eq!(line(&c).as_deref(), Some("working"));
        // Well past the busy timeout, so this is the liveness heartbeat holding
        // the spinner rather than the timeout simply not having come due.
        assert_eq!(
            run(&mut c, t0 + BUSY_TIMEOUT * 3).as_deref(),
            Some("working"),
            "a registered operation keeps its spinner however long it runs"
        );

        c.set(t0, Some("op".into()), StatusTone::Info, "finished");
        assert_eq!(line(&c).as_deref(), Some("finished"));
        assert_eq!(
            run(&mut c, t0).as_deref(),
            Some("meanwhile"),
            "and what waited behind it takes the line with no window to wait out"
        );
    }

    #[test]
    fn the_web_emit_path_is_untouched_by_the_queue() {
        let t0 = Instant::now();
        let mut c = KeyedStatusController::emitting_finals();
        c.set(t0, None, StatusTone::Info, "first");
        c.set(t0, None, StatusTone::Info, "second");
        assert_eq!(
            c.most_recent().map(|s| s.message).as_deref(),
            Some("second"),
            "the web stacks and dismisses; it must stay most-recent-wins"
        );
        c.set(t0, Some("k".into()), StatusTone::Warning, "careful");
        assert_eq!(
            c.snapshot().len(),
            2,
            "a warning must drop nothing on the web"
        );
    }
}
