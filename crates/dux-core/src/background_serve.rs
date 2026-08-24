//! The seam the terminal UI uses to keep a web server serving in its background.
//!
//! ## Why the seam is shaped like this
//!
//! The engine is `!Send` and the terminal UI owns it outright: hundreds of direct
//! `self.engine.*` accesses, some holding borrows across whole render frames. So
//! the web layer cannot own it, cannot borrow it from another thread, and cannot
//! be handed it behind a mutex. What it CAN have is a turn: once per TUI loop
//! iteration, and once per reaction the TUI drains, the TUI lends the engine to a
//! companion and lets it do the web layer's share of the work.
//!
//! ## Crate direction
//!
//! The trait lives here, in `dux-core`, because that is the one crate both the
//! terminal UI and the `dux` binary can see. `dux-tui` never learns that a web
//! layer exists: it calls a trait object. The binary implements the trait over
//! `dux-web`'s serving machinery, and remains the only place the two surfaces
//! meet.
//!
//! ## Accepted hazard: a panic in either surface takes both down
//!
//! While a background server is running, the TUI thread IS the web layer's engine
//! servicer. A panic anywhere in the TUI therefore stops serving, and a panic
//! inside a companion call unwinds through the TUI's run loop. This is written
//! down rather than defended against, because a TUI panic already killed the
//! process (and with it every agent's terminal), so nothing that used to survive
//! stops surviving. What is genuinely new is the reverse direction: a bug in the
//! web layer's per-iteration servicing can now take down a terminal UI that used
//! to be independent of it. That is why the mode is opt-in.

use std::sync::Arc;

use crate::engine::{Engine, EventReaction, PrunedPty};
use crate::pty_owners::PtySizeOwners;

/// What the terminal UI calls itself when it holds a PTY's input.
///
/// Every other participant in the ownership registry is a browser connection and
/// records its raw `User-Agent`; this one is not a browser and has no such thing,
/// so it presents a fixed label instead. It exists to be READ: it is what a
/// watching browser's take-over card names as the device that is driving, so it
/// is written as the copy it becomes rather than as an identifier.
///
/// One label for the whole terminal UI, not one per agent: a pty is driven by a
/// device, and this process is one device.
pub const TUI_DEVICE_LABEL: &str = "the dux TUI";

/// The terminal UI's seat in the PTY-ownership registry, handed to it by the
/// companion for as long as a background server is serving.
///
/// Cheap to hand out (an `Arc` clone and an integer), so the terminal UI asks for
/// it per gesture rather than caching it. That is what makes the toggle-off world
/// byte-identical: nothing is serving, there is no seat, and every gate answers
/// "allowed" without a registry existing at all.
#[derive(Clone)]
pub struct TuiOwnership {
    /// The registry this serve built. One per serve: a stop/start cycle makes a
    /// new one, which is exactly why connection ids are process-global.
    pub owners: Arc<PtySizeOwners>,
    /// The terminal UI's connection id for this serve, drawn from the same
    /// process-global counter every browser socket draws from, so the two are
    /// comparable and can never collide.
    pub conn_id: u64,
}

/// An ownership fact the terminal UI produced, on its way to the browsers.
///
/// The terminal UI can decide these (it holds a seat in the registry) but cannot
/// announce them: the event bus and the per-PTY grid bus are web-layer types on a
/// tokio runtime, and `dux-tui` never sees the web layer at all. So a claim, a
/// release and an applied resize cross the seam as plain data and the binary's
/// companion turns them into the same broadcasts a browser's own claim would have
/// produced. Without that a browser watching a pty the terminal UI just took over
/// would sit on a take-over card naming a device that let go minutes ago.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyOwnershipEvent {
    /// The terminal UI took (or was handed) input ownership of a pty: the
    /// `pty.owner` handover a browser's claim emits, with the same epoch.
    Claimed {
        pty_id: String,
        conn_id: u64,
        epoch: u64,
        /// The claimer's device label, which for this participant is always
        /// [`TUI_DEVICE_LABEL`]. Carried rather than assumed so the translation
        /// on the other side of the seam stays a dumb relay.
        device: String,
    },
    /// The terminal UI let a pty go, and nobody has taken it: the owner-cleared
    /// `pty.owner`. Without it a watcher's card is a permanent lie about a
    /// terminal that stopped serving.
    Released { pty_id: String, epoch: u64 },
    /// A resize the terminal UI owned and APPLIED, stamped with the seq the
    /// owners lock gave it. Web watchers adopt this grid, which is the whole
    /// point of the terminal UI joining the registry: one pty, one authoritative
    /// geometry, whoever is driving.
    GridApplied {
        pty_id: String,
        rows: u16,
        cols: u16,
        seq: u64,
    },
}

/// The results of the shared maintenance sweeps the DRAINER ran this iteration,
/// handed to the companion so browsers learn about them too.
///
/// The sweeps have exactly one runner per process, and while the background
/// server is on that runner is the terminal UI. Everything the web layer's own
/// maintenance would have emitted for them (an "Agent exited." status, a
/// "Terminal closed." status, the spine bump each implies) therefore has to
/// travel this way, or a browser watching an agent die never sees the message and
/// waits for the ~2s fingerprint backstop to notice the row is gone.
#[derive(Debug, Clone, Default)]
pub struct DrainedMaintenance {
    /// The PTYs the drainer's `prune_exited_ptys` reaped this iteration.
    pub pruned: Vec<PrunedPty>,
    /// Whether the drainer's terminal-foreground refresh actually changed a
    /// `foreground_cmd`. A throttled or unchanged probe is `false` and must not
    /// reopen the change gate.
    pub foregrounds_changed: bool,
}

impl DrainedMaintenance {
    /// Nothing swept, so nothing has to cross the seam.
    pub fn is_empty(&self) -> bool {
        self.pruned.is_empty() && !self.foregrounds_changed
    }
}

/// What one turn of the companion's per-iteration servicing did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceOutcome {
    /// A request the companion handled this iteration can have changed shared
    /// workspace state: an agent renamed, a project reordered, a terminal closed.
    ///
    /// The terminal UI reads this to run its OWN post-mutation work (rebuild the
    /// sidebar, clamp the cursors, notice that the selected entity vanished).
    /// Without it a rename made in a browser would leave the TUI's sidebar stale
    /// until something else happened to rebuild it.
    pub mutated: bool,
    /// The companion's request channel closed, or something asked its loop to
    /// stop. Informational: the terminal UI keeps running either way, and the
    /// companion is the one that decides what to do about it.
    pub stopped: bool,
    /// The companion RETIRED itself this iteration: a required listener died, or
    /// its request channel closed, and it has stopped serving on its own.
    ///
    /// Carries the sentence to show the user, because only the companion knows
    /// what died and how; the terminal UI puts it on the status line. Without
    /// this the last thing the user was told is still "serving on ...", which by
    /// then is a lie, and the truth only ever reached `dux.log`.
    pub retirement: Option<String>,
}

/// A web server the terminal UI services once per loop iteration.
///
/// Every method takes `&mut Engine` rather than holding one: the TUI owns the
/// engine and lends it for the duration of the call.
pub trait BackgroundServeCompanion {
    /// Do the companion's share of ONE drained reaction, before the terminal UI
    /// applies it.
    ///
    /// Pre-consume on purpose. `EventReaction` is not `Clone` and the TUI's
    /// `apply_reaction` takes it by value, so a companion that ran afterwards
    /// would have nothing to look at. Per-reaction rather than per-batch for the
    /// same reason, and because it mirrors the order the web layer's own loop has
    /// always fanned reactions out in.
    fn on_reaction(&mut self, engine: &mut Engine, reaction: &EventReaction);

    /// Emit the companion's share of the shared maintenance sweeps the DRAINER
    /// just ran: the exit and close notices, and the change gate they open.
    ///
    /// A narrow lane rather than a second sweep. The drainer already reaped these
    /// PTYs and refreshed these foregrounds (doing it twice would reap twice), so
    /// what crosses is the OUTCOME. Called once per iteration while serving, right
    /// after the drainer's own sweeps.
    fn note_maintenance(&mut self, maintenance: &DrainedMaintenance);

    /// Do the companion's per-iteration work, after the terminal UI has finished
    /// draining. Called once per TUI loop iteration while serving.
    fn service(&mut self, engine: &mut Engine) -> ServiceOutcome;

    /// Note that the terminal UI changed engine state this iteration, so the
    /// companion can open its own change-detection gate. Called with the number of
    /// commands the engine has applied in total; the companion compares it against
    /// what it saw last time.
    fn note_engine_activity(&mut self, command_applies: u64);

    /// Adopt the `[server]` section the terminal UI has just swapped in, so the
    /// handful of settings a running listener can honor take effect.
    ///
    /// Post-apply, unlike [`Self::on_reaction`]: the reload's apply can fail
    /// after validation passed, and a route answering on the incoming caps while
    /// the old config is still in force is worse than one answering a request
    /// late. Called only when the apply succeeded.
    fn note_config_applied(&mut self, server: &crate::config::ServerConfig);

    /// Whether a listener is serving right now.
    fn is_serving(&self) -> bool;

    /// The URLs currently being served, for status copy. Empty when not serving.
    fn urls(&self) -> Vec<String>;

    /// How many browser tabs are connected right now. Zero when not serving.
    ///
    /// "Connections", not "devices", and the terminal UI says so: one browser with
    /// two tabs open is two of these, and nothing on this side of the wire can
    /// honestly tell that they are the same laptop.
    ///
    /// Read once per rendered frame, so it must stay a cheap load rather than a
    /// question that takes a lock or a turn.
    fn connections(&self) -> usize;

    /// Start serving on `listeners`, which the caller already bound.
    ///
    /// Bound by the CALLER so a bind failure is reported without anything having
    /// been torn down: the terminal UI stays exactly where it was. Returns the
    /// URLs on success, or a message fit for the status line.
    fn start(
        &mut self,
        engine: &mut Engine,
        listeners: Vec<std::net::TcpListener>,
        urls: Vec<String>,
    ) -> Result<Vec<String>, String>;

    /// Stop serving and release everything the serve owned. A no-op when not
    /// serving, so a caller never has to check first.
    fn stop(&mut self, engine: &mut Engine);

    /// The terminal UI's seat in this serve's PTY-ownership registry, or `None`
    /// when nothing is serving.
    ///
    /// `None` is the whole toggle-off contract in one value: no seat, no registry
    /// consulted, no gate, and the terminal UI types and resizes exactly the way
    /// it did before any of this existed.
    fn ownership(&self) -> Option<TuiOwnership>;

    /// Announce ownership facts the terminal UI produced: a claim, a release, an
    /// applied resize.
    ///
    /// A relay, not a decision. The registry has already been mutated by the time
    /// these arrive; what is left is telling the browsers, which only the web
    /// layer can do. A no-op when nothing is serving, and cheap to call with an
    /// empty slice.
    fn publish_ownership_events(&mut self, events: &[PtyOwnershipEvent]);
}
