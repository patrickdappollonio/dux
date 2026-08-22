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
//! to be independent of it. The mode is opt-in and marked experimental for that
//! reason among others.

use crate::engine::{Engine, EventReaction};

/// What one turn of the companion's per-iteration servicing did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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

    /// Do the companion's per-iteration work, after the terminal UI has finished
    /// draining. Called once per TUI loop iteration while serving.
    fn service(&mut self, engine: &mut Engine) -> ServiceOutcome;

    /// Note that the terminal UI changed engine state this iteration, so the
    /// companion can open its own change-detection gate. Called with the number of
    /// commands the engine has applied in total; the companion compares it against
    /// what it saw last time.
    fn note_engine_activity(&mut self, command_applies: u64);

    /// Whether a listener is serving right now.
    fn is_serving(&self) -> bool;

    /// The URLs currently being served, for status copy. Empty when not serving.
    fn urls(&self) -> Vec<String>;

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
}
