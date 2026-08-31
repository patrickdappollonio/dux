use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Audience for a status update. `All` (the default) broadcasts to every
/// connected client exactly as statuses behaved before scoping existed;
/// `Connection(id)` restricts delivery to the single web connection whose
/// command originated the operation, so one client's operation toasts
/// (push/commit/launch) stop appearing on every other client.
///
/// Carried from a status's creation all the way to the wire ([`WireStatus`]
/// in `wire.rs`). The TUI ignores it entirely (it has a single status line and
/// a single user); only the web's per-connection status forwarder filters on
/// it. Engine-internal / spontaneous statuses (agent crash, branch move, config
/// reload) and TUI-minted statuses default to `All`.
///
/// [`WireStatus`]: crate::wire::WireStatus
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusScope {
    /// Broadcast to every client (the default, and every pre-scoping status).
    #[default]
    All,
    /// Deliver only to the web connection with this server-assigned id.
    Connection(String),
}

/// Shared timeout for upgrading stale `Busy` entries to `Warning`. Used by
/// both the TUI tick and the web engine actor so the behaviour is identical on
/// both surfaces and the value only lives in one place.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(20);

/// Absolute ceiling on how long liveness may keep a `Busy` on screen.
///
/// [`LiveStatusKeys`] turns the busy timeout from a guess into an answer, but it
/// is a registry, and a registry can leak: an operation whose final never gets
/// produced (a worker path that returns without resolving, a future spawn site
/// that registers and forgets to retire) would otherwise hold a spinner on
/// screen for the life of the process. A spinner must never be literally
/// immortal, so past this age a `Busy` is upgraded whatever liveness says.
///
/// Generous on purpose. It is a backstop for a bug, not a timeout: any real
/// operation dux runs finishes far inside it, and picking a value a slow clone
/// or a long `git fetch` could plausibly cross would reintroduce the false
/// "timed out" this whole mechanism exists to remove.
pub const BUSY_LIVE_CEILING: Duration = Duration::from_secs(30 * 60);

/// The keys of status operations the engine is still running.
///
/// The busy timeout exists to stop a LEAKED spinner claiming forever that work
/// is happening, and from inside [`KeyedStatusController`] it can only ever be a
/// guess about silence: nothing there knows whether a clone on a slow network is
/// thirty seconds into its work or was abandoned. Guessing wrong is not
/// harmless; it replaced a truthful spinner with "timed out" while the clone was
/// still running, and the user watched their agent creation apparently vanish
/// and then succeed minutes later.
///
/// So the answer is recorded instead of inferred, in ONE set rather than per
/// registry. A handle is shared (cheaply cloned) between the engine, which
/// registers a key at the moment it starts the operation behind it, and the
/// surface's controller, which retires the key at the one moment a final lands
/// on it. That pairing is what makes it general: a final of ANY origin, from any
/// of the engine's op registries or from a bare keyed `set`, retires liveness
/// through the same door, and no registry has to be enumerated anywhere.
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

/// How long a FINAL stays replayable under [`StatusRetention::Emit`].
///
/// A reconnect is the same user who watched the spinner start, so it must be
/// told how the operation ended; a page load an hour later is a different
/// session and must not be. This window is what separates the two.
///
/// Deliberately a fixed constant and NOT `ui.status_clear_seconds`. How long a
/// final stays REPLAYABLE and how long a toast stays ON SCREEN are different
/// questions, and that setting answers the second. Tying them would also mean a
/// user who sets `0` (meaning "never auto-clear on screen") gets unbounded
/// retention, which is the stale-replay bug restored through the back door.
///
/// 30 seconds, derived rather than guessed, from the browser's own reconnect
/// budget in `crates/dux-web/web/src/lib/reconnectingSocket.ts`: backoff starts
/// at `RECONNECT_MIN_MS` (500) and doubles, capped at `RECONNECT_MAX_MS` (5000),
/// for at most `MAX_RECONNECT_ATTEMPTS` (3) tries. Three tries are delayed 500,
/// 1000 and 2000 ms, so the cap never even binds and the last attempt starts
/// about 3.5 s after the drop; a socket that has not come back by then has given
/// up, emitted `failed`, and handed the user a Reconnect affordance instead. 30 s
/// clears that with an order of magnitude to spare while staying three orders of
/// magnitude short of the hour-old error this window exists to stop.
///
/// The accepted consequence: a brand-new tab opened inside the window also sees
/// the final. That is fine. A thirty-second-old outcome is current, and it is
/// the price of the reconnect being honest.
pub const FINAL_REPLAY_WINDOW: Duration = Duration::from_secs(30);

/// How many `ui.status_clear_seconds` windows a `Warning` stays up, relative to
/// the single window an `Info` gets.
///
/// Must stay equal to `WARNING_DURATION_FACTOR` in
/// `crates/dux-web/web/src/lib/notify.ts` so the status line and the browser
/// toast agree; `the_web_mirrors_the_warning_clear_factor` reads that file and
/// fails if they drift.
pub const WARNING_CLEAR_FACTOR: u32 = 3;

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
    /// Deliberately ORTHOGONAL to tone. A catastrophic error is still visually
    /// an error, so a `Critical` tone would have put every call site on a
    /// spectrum with no clear line and duplicated every icon and colour
    /// decision. This flag answers one crisp question instead: does this
    /// message wait for the user, or does it leave on its own?
    ///
    /// The rule for setting it, and it is meant to stay rare: the user must act
    /// OUTSIDE the toast to recover, or something may have been lost or left
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
    /// The re-broadcast is not decoration. A browser holds its own leak guard on
    /// every spinner (`BUSY_TOAST_MAX_MS` in
    /// `crates/dux-web/web/src/lib/notify.ts`), and the only thing that re-arms
    /// it is another frame on the same key. Keeping the entry alive server-side
    /// while saying nothing on the wire would move the silent disappearance from
    /// the engine to the browser rather than fix it.
    pub refreshed: Vec<KeyedWireStatus>,
    /// How many finals aged out of the replay window under
    /// [`StatusRetention::Emit`]. These are deliberately NOT reported as keys:
    /// the caller must refresh its published snapshot but must send NO frame for
    /// them, because a `status_cleared` would dismiss the toast on every screen
    /// showing it, `sticky` ones included. A count rather than a list, so there
    /// is nothing here to accidentally turn into frames.
    pub purged: usize,
}

/// What the controller does with a FINAL status (anything that is not
/// [`StatusTone::Busy`]: info/success, warning, error).
///
/// The distinction exists because a `Busy` is live STATE while a final is an
/// EVENT. A surface that can be joined late (the web, where every page load and
/// every reconnect replays the snapshot) must be told about work still in
/// flight, and must not still be told, an hour on, about an outcome that is
/// long over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusRetention {
    /// Store a final and keep it until something replaces it. The TUI's single
    /// status line has no other way to show an outcome, and it is never
    /// "reconnected", so the last message simply stays on screen.
    Retain,
    /// Treat a final as an event with a short tail: it is broadcast live by the
    /// caller (the web emitter sends the `WireStatus` it was given), kept
    /// replayable for [`FINAL_REPLAY_WINDOW`], and then dropped by
    /// [`tick`](KeyedStatusController::tick).
    ///
    /// The window is not a detail, it is the point. Without it a socket that
    /// drops while an operation is running comes back to a snapshot holding
    /// NOTHING: the busy was retired by its own final and the final was
    /// broadcast to nobody, so the client sits on a spinner until its leak guard
    /// silently retires it and the user is never told the operation failed. With
    /// it, the reconnecting tab is handed the final it missed and the hour-old
    /// error still never comes back.
    ///
    /// The expiry is SILENT: no cleared key is reported, because the on-screen
    /// lifetime belongs to the client (which retires each toast on its own
    /// timer, and deliberately never retires a `sticky` one). A `status_cleared`
    /// at the thirty-second mark would dismiss exactly the messages that were
    /// marked as needing to wait for the user.
    Emit,
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
    /// What happens to a final (non-`Busy`) status. See [`StatusRetention`].
    retention: StatusRetention,
    /// Which keys still have an operation running behind them. The controller
    /// READS it to decide whether a timed-out busy is stranded or merely slow,
    /// and RETIRES a key whenever a final lands on it, which is the one place
    /// every final of every origin passes through.
    ///
    /// Default-empty, so a controller nobody handed a shared set to (every test
    /// that does not care, and any future surface before it is wired) behaves
    /// exactly as it did before liveness existed.
    live: LiveStatusKeys,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusTickAction {
    Keep,
    Clear,
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
            BusyUpgrade::TimedOut => "timed out — check dux.log".to_string(),
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
    clear: Vec<String>,
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

    /// The generation of the message currently on the anonymous slot, or
    /// `None` when the slot is empty.
    ///
    /// The anonymous slot is most-recent-wins and shared by every unkeyed
    /// producer, so "is my message still the one on the line" cannot be
    /// answered by tone (several producers write warnings) or by text
    /// (comparing strings would match a second producer's identical message).
    /// A producer that wants to retire its own message keeps the generation its
    /// `set` returned and clears only while this still equals it.
    pub fn anon_generation(&self) -> Option<Generation> {
        self.anon.as_ref().map(|a| a.generation)
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

        // Both policies STORE the entry, including a final: `Emit` differs only
        // in how long it keeps one, which is [`tick`](Self::tick)'s job. Storing
        // is also what retires the `Busy` the final replaces, so no spinner is
        // ever left behind on the slot.
        match key {
            None => {
                self.anon = Some(entry);
                // A new anonymous set always clears the pin so the new message
                // follows normal auto-clear rules (the pin was for the old one).
                self.anon_pinned = false;
            }
            Some(k) => {
                // A final on a key is the one moment every finished operation
                // passes through, whichever registry (or none) it came from, so
                // this is where liveness is retired. A `Busy` is the opposite
                // signal and leaves the registration alone: `progress` re-emits
                // one on the same key mid-operation.
                if tone != StatusTone::Busy {
                    self.live.retire(&k);
                }
                self.entries.insert(k, entry);
            }
        }

        generation
    }

    /// Remove a keyed entry IFF the carried generation matches the stored one
    /// (the clear-race guard) AND the entry is not [`sticky`].
    ///
    /// - `generation == None` skips the generation check (but NOT the sticky
    ///   one).
    ///
    /// A STICKY entry is never removed by a clear, whichever form is used. A
    /// clear means "the operation ended with nothing to say"; a sticky final
    /// means "something is half-done and is waiting for you". Those cannot both
    /// be true of one key, and the sticky final is the newer, more specific
    /// fact. Without this guard `sticky` would be decorative for every
    /// engine-raised status, because a clear names nothing but a key and every
    /// keyed final is reachable by one. If an operation genuinely supersedes a
    /// sticky message it must SAY so with a new [`set`](Self::set) on the key,
    /// which still replaces it; silently removing the message is not an option
    /// the user can act on.
    ///
    /// This cannot strand anything. Under [`StatusRetention::Emit`] the replay
    /// window in [`tick`](Self::tick) still retires a sticky entry, and it does
    /// so silently, so the snapshot does not grow and the toast on screen is
    /// left for the user to dismiss.
    ///
    /// Returns `true` if anything was removed, so a caller that broadcasts a
    /// dismissal only does it when one actually happened.
    ///
    /// [`sticky`]: KeyedWireStatus::sticky
    pub fn clear(&mut self, key: &str, generation: Option<Generation>) -> bool {
        // A clear is a final that had nothing to say, so it retires liveness the
        // same way a message does. Unconditionally, and before the sticky and
        // generation guards: those decide what stays ON SCREEN, while this
        // records that the operation is over.
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
                self.entries.swap_remove(key);
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
    ///   `Warning`, never for `Error`) is removed AND reported in
    ///   `cleared_keys`. A `sticky` final and the pinned anonymous slot are
    ///   exempt, because both wait for the user rather than for a timer.
    ///
    /// Under [`StatusRetention::Emit`]:
    /// - EVERY final (info, warning, error alike) older than
    ///   [`FINAL_REPLAY_WINDOW`] is removed SILENTLY, with nothing reported. It
    ///   is leaving the replay snapshot, not leaving the user's screen, and
    ///   `clear_after` plays no part.
    ///
    /// Under both:
    /// - `Busy` entries silent for longer than `busy_timeout` are upgraded
    ///   in-place to a `Warning`, so a leaked busy is never immortal. The
    ///   upgrade restamps `since`, so under `Emit` the resulting warning gets
    ///   its own full replay window before it ages out. It is upgraded to a
    ///   `Warning` with a "timed out" message, UNLESS [`LiveStatusKeys`] says an
    ///   operation is still registered behind the key, in which case the entry
    ///   is re-stamped and handed back in `refreshed` for re-broadcast. A slow
    ///   clone therefore keeps its spinner for as long as it takes, and the
    ///   timeout still fires for a spinner nothing is behind. Past
    ///   [`BUSY_LIVE_CEILING`] the upgrade happens whatever liveness says.
    ///
    /// Returns the set of changes the caller must broadcast.
    pub fn tick(&mut self, now: Instant, busy_timeout: Duration) -> StatusTickChanges {
        let mut changes = StatusTickChanges::default();
        self.tick_anonymous(now, busy_timeout, &mut changes);
        let actions = self.keyed_tick_actions(now, busy_timeout);
        self.clear_keyed_finals(actions.clear, &mut changes);
        self.purge_keyed_finals(actions.purge, &mut changes);
        self.upgrade_keyed_busys(actions.upgrade, now, &mut changes);
        self.stall_keyed_busys(actions.stalled, now, &mut changes);
        self.heartbeat_keyed_busys(actions.heartbeat, now, &mut changes);

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
        anon.message = "timed out — check dux.log".to_string();
        anon.since = now;
        anon.heartbeat = now;
        anon.generation = Generation(self.next_gen);
        anon.seq = self.next_seq;
        self.next_gen += 1;
        self.next_seq += 1;
        changes.upgraded.push(KeyedWireStatus {
            key: None,
            tone: StatusTone::Warning.as_wire().to_string(),
            message: "timed out — check dux.log".to_string(),
            scope: anon.scope.clone(),
            sticky: false,
        });
    }

    fn keyed_tick_actions(&self, now: Instant, busy_timeout: Duration) -> KeyedTickActions {
        let mut actions = KeyedTickActions::default();
        for (key, entry) in &self.entries {
            match self.keyed_tick_action(key, entry, now, busy_timeout) {
                StatusTickAction::Keep => {}
                StatusTickAction::Clear => actions.clear.push(key.clone()),
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
        if !entry.sticky
            && !self.clear_after.is_zero()
            && let Some(windows) = entry.tone.clear_windows()
            && age >= self.clear_after * windows
        {
            return StatusTickAction::Clear;
        }
        StatusTickAction::Keep
    }

    fn clear_keyed_finals(&mut self, keys: Vec<String>, changes: &mut StatusTickChanges) {
        for key in keys {
            self.entries.swap_remove(&key);
            changes.cleared_keys.push(Some(key));
        }
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
    /// and the spinner animation and the TUI's most-recent-wins ordering both
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
                key: Some(key),
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
            key: Some(key.to_string()),
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
                key: Some(entry.key.clone().unwrap_or_default()),
                tone: entry.tone.as_wire().to_string(),
                message: entry.message.clone(),
                scope: entry.scope.clone(),
                sticky: entry.sticky,
            });
        }
        out
    }

    /// Select the most-recently-set open status. Sequence numbers break ties
    /// between entries written at the same instant.
    fn most_recent_entry(&self) -> Option<&KeyedStatus> {
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

    /// The single line the TUI shows: the most-recently-set open status (keyed
    /// or anonymous), or `None` when nothing is open.
    ///
    /// When two entries share the same `since` timestamp the one with the
    /// higher sequence number wins (the later `set` call).
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
        LiveStatusKeys, StatusTone,
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

        assert_eq!(
            changes.cleared_keys,
            vec![None, Some("clear-a".into()), Some("clear-b".into())]
        );
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
        let anon = c.anon.as_ref().expect("anonymous upgrade");
        assert_eq!(anon.generation, super::Generation(2));
        assert_eq!(anon.seq, 2);
        assert!(anon.sticky, "the anonymous stored entry keeps its flag");
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
        c.set(t0, None, StatusTone::Warning, "Already serving.");
        c.set(
            t0,
            Some("push".into()),
            StatusTone::Warning,
            "Push is stale.",
        );
        c.set(t0, Some("pull".into()), StatusTone::Error, "Pull failed.");

        // One window in, a warning is still there: it outlives an info.
        let changes = c.tick(t0 + window, BUSY_TIMEOUT);
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        assert_eq!(c.snapshot().len(), 3);

        // A second short of three windows still keeps them.
        let changes = c.tick(t0 + window * 3 - Duration::from_secs(1), BUSY_TIMEOUT);
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        assert_eq!(c.snapshot().len(), 3);

        // At three windows both warnings go, announced, and the error stays.
        let changes = c.tick(t0 + window * 3, BUSY_TIMEOUT);
        assert!(changes.cleared_keys.contains(&None));
        assert!(changes.cleared_keys.contains(&Some("push".to_string())));
        assert_eq!(changes.cleared_keys.len(), 2);
        let snap = c.snapshot();
        assert_eq!(snap.len(), 1, "only the error survives: {snap:?}");
        assert_eq!(snap[0].key.as_deref(), Some("pull"));

        // And the error is still there an hour later.
        let _ = c.tick(t0 + Duration::from_secs(3600), BUSY_TIMEOUT);
        assert_eq!(c.snapshot().len(), 1);
    }

    #[test]
    fn a_zero_window_keeps_warnings_too() {
        // `status_clear_seconds = 0` means "never auto-clear", for every tone.
        let t0 = Instant::now();
        let mut c = KeyedStatusController::with_clear_after(Duration::ZERO);
        c.set(t0, None, StatusTone::Warning, "Already serving.");
        c.set(t0, Some("save".into()), StatusTone::Info, "Saved.");
        let changes = c.tick(t0 + Duration::from_secs(3600), BUSY_TIMEOUT);
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        assert_eq!(c.snapshot().len(), 2);
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
        // A warning outlasts the info window; an error outlasts everything.
        c.set(t0, Some("stale".into()), StatusTone::Warning, "Heads up.");
        c.set(t0, Some("push".into()), StatusTone::Error, "Push error.");
        let changes = c.tick(t0 + Duration::from_secs(6), Duration::from_secs(20));
        // Both Info entries cleared; the warning and the error persist.
        assert_eq!(changes.cleared_keys.len(), 2);
        assert!(changes.cleared_keys.contains(&None));
        assert!(changes.cleared_keys.contains(&Some("commit".to_string())));
        assert_eq!(c.snapshot().len(), 2);

        // The warning is still on the line at seventeen seconds and gone at
        // eighteen, three times the six-second window.
        let changes = c.tick(t0 + Duration::from_secs(17), Duration::from_secs(20));
        assert!(changes.cleared_keys.is_empty(), "{changes:?}");
        let changes = c.tick(t0 + Duration::from_secs(18), Duration::from_secs(20));
        assert_eq!(changes.cleared_keys, vec![Some("stale".to_string())]);
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
        assert_eq!(changes.upgraded[0].message, "timed out — check dux.log");
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
        c.set_scoped(
            t0,
            Some("halfdone".into()),
            T::Error,
            "Worktree delete failed.",
            StatusScope::All,
            true,
        );
        let snap = c.snapshot();
        let ordinary = snap
            .iter()
            .find(|e| e.key.as_deref() == Some("ordinary"))
            .expect("ordinary entry");
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
}
