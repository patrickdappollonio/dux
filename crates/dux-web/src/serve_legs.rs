//! Per-leg listener lifecycle: the shutdown primitive every serve path shares,
//! the registry of live legs and their individual stop lanes, and the Tailscale
//! interface watcher that adds and drops the Tailscale leg while dux keeps
//! serving.
//!
//! dux serves one router on several listeners: the REQUIRED one the operator named
//! and the BEST-EFFORT Tailscale one. A required listener dying means the server is
//! over; the Tailscale one dying is a suspended laptop or a restarted daemon. So
//! each listener gets its own stop lane and its own failure verdict, and one leg can
//! end without taking the server with it.
//!
//! - A PARENT trip (a signal, a required leg's death, the flip's engine loop
//!   returning) fans out over every leg lane, so nothing holds a socket afterwards.
//! - A LEG trip stops exactly one listener and leaves the parent alone.
//!
//! Every serve future waits on both its own lane and the parent's.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dux_core::config::{TailscaleMode, TailscaleModeOutcome};
use dux_core::tailscale::{TailscaleIdentity, TailscaleUnavailable};

/// How often the watcher asks whether the Tailscale address is there. A constant
/// rather than a setting: an implementation cadence, well inside the time a person
/// takes to notice a laptop is back and reach for a browser, paying for one bounded
/// local call per period.
///
/// A look is one `tailscale ip`, measured at 5 to 19 ms on the machine this was
/// written on, so looking this often costs nothing worth naming.
///
/// This period is also the flap debounce, and there is deliberately no second
/// hysteresis window: an interface appearing and disappearing faster than this
/// produces at most one transition per period, and one slower than this is not
/// flapping, it is changing.
pub(crate) const WATCH_PERIOD: Duration = Duration::from_secs(5);

/// How long the watcher parks between checks of the stop flag. Small enough that
/// serving can end promptly, large enough that waiting costs a wakeup a second
/// and nothing else.
const WATCH_SLICE: Duration = Duration::from_millis(250);

/// The one serve-shutdown primitive every serve path shares: first-error
/// bookkeeping, the parent lane every listener awaits, and the per-leg lanes, so a
/// dying listener winds its siblings down identically everywhere while a
/// best-effort leg can still be stopped alone.
///
/// - `failed` is armed once (compare-exchange) so the FIRST failing REQUIRED
///   listener is the one that records the returned error and is reported.
/// - `error` holds that first error, surfaced to the caller after wind-down.
/// - `shutdown_tx` is the parent lane, flipped on a required failure or a normal
///   SIGINT/SIGTERM. Tripping it stops the whole server, legs included.
/// - `legs` maps each live listener's address to its own stop lane.
/// - `tailscale_watched` records whether an interface watcher is running for this
///   serve, which is the one thing a dying best-effort leg needs to know to say
///   truthfully whether it is coming back.
#[derive(Clone)]
pub(crate) struct ServeShutdown {
    failed: Arc<AtomicBool>,
    error: Arc<std::sync::Mutex<Option<anyhow::Error>>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    legs: Arc<std::sync::Mutex<HashMap<SocketAddr, tokio::sync::watch::Sender<bool>>>>,
    tailscale_watched: Arc<AtomicBool>,
}

impl ServeShutdown {
    /// `tailscale_watched` is the serve's live watcher flag, which starts at
    /// [`TailscaleMode::watches_interface`] for this run and moves with a live
    /// mode change. The registry deliberately knows nothing about config modes,
    /// so the serve path states the fact once here instead of every leg
    /// restating it.
    pub(crate) fn new(tailscale_watched: Arc<AtomicBool>) -> Self {
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        Self {
            failed: Arc::new(AtomicBool::new(false)),
            error: Arc::new(std::sync::Mutex::new(None)),
            shutdown_tx,
            legs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            tailscale_watched,
        }
    }

    /// A shutdown primitive for a serve with no live mode control behind it
    /// (tests, and any path that never changes the mode).
    #[cfg(test)]
    pub(crate) fn for_watched(watched: bool) -> Self {
        Self::new(Arc::new(AtomicBool::new(watched)))
    }

    /// A fresh receiver on the parent lane.
    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    /// Whether a REQUIRED serve task has recorded a failure (polled by the flip's
    /// engine-loop control closure to exit the loop). A best-effort leg's death
    /// never arms this.
    pub(crate) fn is_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// Register a leg and return its own stop lane receiver. Registering an
    /// address that is somehow already registered replaces the old lane after
    /// tripping it, so no listener is ever left with nothing able to stop it.
    pub(crate) fn register_leg(&self, addr: SocketAddr) -> tokio::sync::watch::Receiver<bool> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        if let Ok(mut legs) = self.legs.lock()
            && let Some(previous) = legs.insert(addr, tx)
        {
            let _ = previous.send(true);
        }
        rx
    }

    /// Stop ONE leg: trip its lane and forget it. Returns whether a live leg was
    /// there to stop. The parent lane is untouched, which is the whole point.
    pub(crate) fn stop_leg(&self, addr: SocketAddr) -> bool {
        let Ok(mut legs) = self.legs.lock() else {
            return false;
        };
        match legs.remove(&addr) {
            Some(tx) => {
                let _ = tx.send(true);
                true
            }
            None => false,
        }
    }

    /// Forget a leg whose task has already ended, without tripping anything.
    pub(crate) fn forget_leg(&self, addr: SocketAddr) {
        if let Ok(mut legs) = self.legs.lock() {
            legs.remove(&addr);
        }
    }

    /// Every address a leg is serving right now, loopback first and then in
    /// address order. `None` means the registry could NOT be read, which is a
    /// different answer from an empty list: a serve whose legs have all gone
    /// really is reachable nowhere, and a caller that treats the two alike shows
    /// an address nothing is listening on.
    ///
    /// The registry is the one live answer, which is what makes it the right
    /// source for an address list a surface keeps on screen: the list handed to a
    /// serve at start is a snapshot of that moment, and the Tailscale leg comes
    /// and goes underneath it.
    pub(crate) fn leg_addrs(&self) -> Option<Vec<SocketAddr>> {
        let legs = self.legs.lock().ok()?;
        let mut addrs: Vec<SocketAddr> = legs.keys().copied().collect();
        addrs.sort_by_key(|addr| (!addr.ip().is_loopback(), addr.to_string()));
        Some(addrs)
    }

    /// Poison the leg registry's lock, so a test can tell "could not read" from
    /// "nothing registered" without waiting for a real panic to do it.
    #[cfg(test)]
    pub(crate) fn poison_legs_for_test(&self) {
        let _guard = self.legs.lock();
        panic!("poisoning the leg registry on purpose");
    }

    /// Whether a live leg is registered for `addr`. The registry is the ONE answer
    /// to "is dux actually serving this address", so the serve loop reconciles its
    /// watcher-facing bookkeeping against it rather than keeping a second truth.
    pub(crate) fn has_leg(&self, addr: SocketAddr) -> bool {
        self.legs
            .lock()
            .map(|legs| legs.contains_key(&addr))
            .unwrap_or(false)
    }

    /// Trigger a graceful, non-error wind-down of the WHOLE server: the parent
    /// lane plus every registered leg lane. Fanning out matters because a leg
    /// added after the serve started (the Tailscale watcher's doing) waits on its
    /// own lane, and a teardown that only tripped the parent would leave that
    /// listener holding its socket into whatever came next. Idempotent.
    pub(crate) fn trigger(&self) {
        let _ = self.shutdown_tx.send(true);
        if let Ok(mut legs) = self.legs.lock() {
            for (_, tx) in legs.drain() {
                let _ = tx.send(true);
            }
        }
    }

    /// Take the first recorded serve error, if any.
    pub(crate) fn take_error(&self) -> Option<anyhow::Error> {
        self.error.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Record a REQUIRED serve task's failure exactly once and wind the whole
    /// server down. The FIRST caller wins: it stores the error and is the one
    /// reported; later callers no-op the error slot. Always trips the parent lane
    /// (and therefore every leg) so the remaining listeners stop too. Returns
    /// `true` when this call was the first-error winner.
    pub(crate) fn record_failure(&self, err: anyhow::Error) -> bool {
        let first = self
            .failed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        if first && let Ok(mut slot) = self.error.lock() {
            *slot = Some(err);
        }
        self.trigger();
        first
    }

    /// Record a BEST-EFFORT leg's death: log it, mark the leg down, and let the
    /// server carry on. Deliberately NOT a parent trip and NOT an error: the
    /// whole reason a leg is best-effort is that losing it is not losing the
    /// server, and the Tailscale leg is the one users lose routinely.
    ///
    /// Answers with the sentence it logged, because the caller owes the same one
    /// to the surfaces and computing it twice is how the two drift apart.
    pub(crate) fn record_best_effort_failure(
        &self,
        addr: SocketAddr,
        err: &anyhow::Error,
    ) -> String {
        let warning =
            best_effort_death_warning(addr, err, self.tailscale_watched.load(Ordering::SeqCst));
        dux_core::logger::warn(&format!("[server] {warning}"));
        self.forget_leg(addr);
        warning
    }
}

/// The keyed-status key the Tailscale leg's health is reported under.
///
/// One key for the whole leg, deliberately: a leg that comes back is the answer
/// to the warning that it went away, and on one key the good news replaces the
/// bad rather than sitting under it.
pub(crate) const TAILSCALE_LEG_KEY: &str = "tailscale-leg";

/// The lane a serve uses to tell BOTH surfaces about its Tailscale leg.
///
/// The console is `dux server`'s terminal and nothing else: a leg that died
/// while the background server serves under a terminal UI wrote to a console
/// nobody was looking at, and the browsers were never told at all. The engine's
/// worker lane is the one road to both, so these facts travel it as well as the
/// console line, which stays exactly as it was.
#[derive(Clone, Default)]
pub(crate) struct LegStatus {
    handle: Option<crate::engine_actor::EngineHandle>,
    /// Shared by every clone, because the clone a leg task carries reports the
    /// same leg as the serve loop's own and the two must coalesce together.
    coalescer: Arc<std::sync::Mutex<LegCoalescer>>,
}

/// How long the leg must hold a new state before dux says it changed.
///
/// One second longer than one [`WATCH_PERIOD`], which is what makes it a dwell
/// rather than a second name for the watcher's cadence: an interface that has
/// not outlived a whole period is one the watcher is still changing its mind
/// about. A wedged probe stretches a cycle past the dwell, which only costs the
/// coalescing, never a wrong sentence.
///
/// The console line and the log stay immediate; this delays only what the
/// surfaces are told, because every sentence lands on one key and a toast
/// re-raised on a fixed id restarts its window instead of expiring.
pub(crate) const LEG_SETTLE_DWELL: Duration =
    Duration::from_secs(WATCH_PERIOD.as_secs().saturating_add(1));

/// A leg transition waiting out its dwell.
struct PendingLegTransition {
    due: std::time::Instant,
    tone: dux_core::statusline::StatusTone,
    message: String,
}

/// Collapses a flapping leg into one sentence: nothing is said until a state has
/// held for [`LEG_SETTLE_DWELL`], and a transition that lands inside another's
/// dwell says once that the interface is unstable.
#[derive(Default)]
struct LegCoalescer {
    pending: Option<PendingLegTransition>,
    /// Whether the unstable sentence has already gone out for this spell of
    /// flapping, so the same key is not re-raised every few seconds.
    unstable: bool,
}

impl LegCoalescer {
    /// Record one transition. Answers with what to say right now, which is
    /// nothing at all unless this is the second transition of a flap.
    fn record(
        &mut self,
        now: std::time::Instant,
        tone: dux_core::statusline::StatusTone,
        message: &str,
    ) -> Option<dux_core::engine::StatusUpdate> {
        let interrupted = self.pending.is_some();
        self.pending = Some(PendingLegTransition {
            due: now + LEG_SETTLE_DWELL,
            tone,
            message: message.to_string(),
        });
        if !interrupted || self.unstable {
            return None;
        }
        self.unstable = true;
        Some(leg_status_update(
            dux_core::statusline::StatusTone::Warning,
            &leg_unstable_warning(),
        ))
    }

    /// Answers with the transition whose dwell has elapsed, if one has.
    fn due(&mut self, now: std::time::Instant) -> Option<dux_core::engine::StatusUpdate> {
        if self
            .pending
            .as_ref()
            .is_none_or(|pending| now < pending.due)
        {
            return None;
        }
        let pending = self.pending.take()?;
        if std::mem::take(&mut self.unstable) {
            return Some(leg_status_update(
                pending.tone,
                &leg_settled_message(&pending.message),
            ));
        }
        Some(leg_status_update(pending.tone, &pending.message))
    }

    fn deadline(&self) -> Option<std::time::Instant> {
        self.pending.as_ref().map(|pending| pending.due)
    }
}

fn leg_status_update(
    tone: dux_core::statusline::StatusTone,
    message: &str,
) -> dux_core::engine::StatusUpdate {
    dux_core::engine::StatusUpdate::keyed(TAILSCALE_LEG_KEY, tone, message)
}

/// What dux says while the interface is coming and going faster than it can be
/// reported one change at a time.
pub(crate) fn leg_unstable_warning() -> String {
    format!(
        "The Tailscale interface is going up and down: dux is binding and dropping that address \
         as it comes and goes, and is still serving on its other address(es). dux says what \
         happened once it has stayed one way for {}s.",
        LEG_SETTLE_DWELL.as_secs()
    )
}

/// And what it says once the flapping stops, on the same key, so the warning is
/// answered rather than left standing.
pub(crate) fn leg_settled_message(message: &str) -> String {
    format!("The Tailscale interface has settled. {message}")
}

impl LegStatus {
    pub(crate) fn new(handle: crate::engine_actor::EngineHandle) -> Self {
        Self {
            handle: Some(handle),
            coalescer: Arc::new(std::sync::Mutex::new(LegCoalescer::default())),
        }
    }

    /// The leg degraded: it could not bind, or it stopped serving. A warning,
    /// because the address the user may be reaching dux on has gone.
    pub(crate) fn degraded(&self, message: &str) {
        self.record(
            std::time::Instant::now(),
            dux_core::statusline::StatusTone::Warning,
            message,
        );
    }

    /// The leg arrived or left as the interface came and went. An info, for the
    /// same reason the console says it in the quiet tone: it is expected news.
    pub(crate) fn changed(&self, message: &str) {
        self.record(
            std::time::Instant::now(),
            dux_core::statusline::StatusTone::Info,
            message,
        );
    }

    fn record(
        &self,
        now: std::time::Instant,
        tone: dux_core::statusline::StatusTone,
        message: &str,
    ) {
        let update = self.lock().record(now, tone, message);
        if let Some(update) = update {
            self.post(update);
        }
    }

    /// Say the transition whose dwell has elapsed. The serve loop calls this on
    /// its own clock, which is the only thing that ever moves a held sentence.
    pub(crate) fn flush_due(&self, now: std::time::Instant) {
        let update = self.lock().due(now);
        if let Some(update) = update {
            self.post(update);
        }
    }

    /// When the held transition is due, for the serve loop's timer arm.
    pub(crate) fn pending_deadline(&self) -> Option<std::time::Instant> {
        self.lock().deadline()
    }

    /// Say something about this machine's tailnet NAME (a rename, a serve route,
    /// Funnel). Posted at once on its own key: these are not interface flaps, so
    /// nothing here is held for a dwell.
    pub(crate) fn identity_news(&self, tone: dux_core::statusline::StatusTone, message: &str) {
        self.post(dux_core::engine::StatusUpdate::keyed(
            TAILSCALE_IDENTITY_KEY,
            tone,
            message,
        ));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LegCoalescer> {
        self.coalescer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn post(&self, status: dux_core::engine::StatusUpdate) {
        if let Some(handle) = &self.handle {
            handle.post_status(status);
        }
    }
}

/// What to say when a BEST-EFFORT (Tailscale) leg's accept loop dies mid-run.
///
/// `watched` is what makes the second half honest. On `auto` a watcher is running
/// and the serve loop clears its bound bookkeeping when the leg's task ends, so
/// the next watch period really does bind it again; on `yes` nothing is watching
/// and the leg is down for the rest of the run, so the message has to name the
/// two ways out instead of promising a recovery that never arrives.
pub(crate) fn best_effort_death_warning(
    addr: SocketAddr,
    err: &anyhow::Error,
    watched: bool,
) -> String {
    let recovery = if watched {
        format!(
            "dux is watching the Tailscale interface, so it binds this address again by itself \
             within about {}s of the interface being back; nothing to do.",
            WATCH_PERIOD.as_secs()
        )
    } else {
        "[server] tailscale = \"yes\" looks for the Tailscale address exactly once, at startup, \
         so this leg stays down for the rest of this run: restart dux to bind it again, or set \
         [server] tailscale to \"auto\" to have dux bind and drop it as the interface comes and \
         goes."
            .to_string()
    };
    format!(
        "the listener on the Tailscale address {addr} stopped serving: {err}. dux is still \
         serving on its other address(es). {recovery}"
    )
}

/// Await either the parent lane or this leg's own lane, whichever trips first.
/// Every serve future waits on both, so a per-leg stop and a whole-server
/// teardown both reach it. A wakeup-driven await, no sleep-poll.
pub(crate) async fn wait_for_leg_shutdown(
    parent: tokio::sync::watch::Receiver<bool>,
    leg: tokio::sync::watch::Receiver<bool>,
) {
    tokio::select! {
        _ = wait_for_shutdown(parent) => {},
        _ = wait_for_shutdown(leg) => {},
    }
}

/// Await one shutdown lane: resolve once the watch flips to `true`. The receiver
/// is consumed, so each caller passes its own handle.
pub(crate) async fn wait_for_shutdown(mut rx: tokio::sync::watch::Receiver<bool>) {
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            break;
        }
    }
}

// ── The Tailscale watcher ──────────────────────────────────────────────────

/// What the watcher asks the serve loop to do with the LEG. One command per
/// detect period at most, except a genuine address CHANGE, which is an unbind and
/// a bind of the same leg and is sent as both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegCommand {
    /// The Tailscale interface is there at this address: bind and serve it,
    /// best-effort.
    Bind(SocketAddr),
    /// This Tailscale address is gone: stop that listener. Live sockets on it die
    /// with the listener, and the browser's ordinary reconnect is the recovery.
    Unbind(SocketAddr),
}

/// Everything the watcher tells the serve loop: a leg command, or a new
/// identity, sent only when it differs from the one the loop already holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WatchEvent {
    Leg(LegCommand),
    /// This machine's name on the tailnet, or the `tailscale serve` routes that
    /// end at dux, changed: a first look, a tailnet rename, a serve route added
    /// or removed. The serve loop moves the Host guard and the shown URLs.
    Identity(TailscaleIdentity),
    /// A look at the identity FAILED, and why. The serve loop stops admitting
    /// the name until a look succeeds again (the look that failed is the one
    /// that would have seen a Funnel switched on), and the reason decides the
    /// Funnel lockout: no CLI or no daemon means nothing can be published, while
    /// a CLI that fails leaves it unknown.
    IdentityFailed(TailscaleUnavailable),
}

/// The step one detect period implies, given what is bound now and what was just
/// detected. Pure, so every transition (including the ones that are hard to
/// arrange with a real interface) is a unit test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegStep {
    /// Nothing changed. The overwhelmingly common case.
    Nothing,
    Bind(SocketAddr),
    Unbind(SocketAddr),
    /// The Tailscale address itself changed. Both halves are sent in this one
    /// period, because leaving a listener on an address the machine no longer has
    /// is worse than doing two things at once.
    Rebind {
        old: SocketAddr,
        new: SocketAddr,
    },
}

/// Decide the step from the currently bound leg and the desired one.
pub(crate) fn plan_leg_step(bound: Option<SocketAddr>, desired: Option<SocketAddr>) -> LegStep {
    match (bound, desired) {
        (None, None) => LegStep::Nothing,
        (None, Some(new)) => LegStep::Bind(new),
        (Some(old), None) => LegStep::Unbind(old),
        (Some(old), Some(new)) if old == new => LegStep::Nothing,
        (Some(old), Some(new)) => LegStep::Rebind { old, new },
    }
}

/// The address the Tailscale leg WANTS to be at, given a detection result and the
/// primary listener, or `None` when there should be no leg.
///
/// A detected address is refused as a leg when the primary listener already
/// covers it: a wildcard primary (`0.0.0.0` / `::`) is listening on every
/// interface including this one, and a primary bound to the Tailscale address
/// itself plainly is it. Binding a second listener on the same address would just
/// fail with EADDRINUSE once a period, forever.
pub(crate) fn desired_leg(
    primary: SocketAddr,
    detected: Result<IpAddr, TailscaleUnavailable>,
) -> Option<SocketAddr> {
    let ip = detected.ok()?;
    if primary.ip().is_unspecified() || primary.ip() == ip {
        return None;
    }
    Some(SocketAddr::new(ip, primary.port()))
}

/// One step of a live `[server] tailscale` mode change, decided by
/// [`plan_mode_change`] and carried out by the serve loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModeStep {
    /// End the watcher that is running, so its in-flight probe cannot re-bind a
    /// leg the new mode does not want.
    StopWatcher,
    /// Drop the Tailscale listener at this address.
    Unbind(SocketAddr),
    /// Look for the Tailscale address once and bind whatever that implies.
    DetectAndBind,
    /// Start a watcher for the rest of the serve. `probe_now` skips its first
    /// park so the command that asked for `auto` has a visible outcome.
    StartWatcher { probe_now: bool },
    /// Accept (or stop accepting) Tailscale IP literals in the Host guard.
    SetHostLiterals(bool),
    /// Let go of this machine's tailnet name: the Host guard stops admitting it
    /// and no surface offers a URL built on it. A later mode that wants
    /// Tailscale reads it afresh.
    ForgetIdentity,
    /// This run was started with `--no-tailscale`, so nothing is done to the
    /// listeners.
    Refuse,
}

/// The steps a live mode change implies. Pure, so the whole transition matrix is
/// a unit test rather than nine socket-holding integration cases.
///
/// The Host-literal step comes BEFORE the bind on the way up and AFTER the
/// unbind on the way down, so there is never a window where dux is serving a
/// Tailscale address its own Host guard refuses.
pub(crate) fn plan_mode_change(
    prev: TailscaleMode,
    next: TailscaleMode,
    bound: Option<SocketAddr>,
    forced_no: bool,
) -> Vec<ModeStep> {
    if forced_no && next.wants_tailscale() {
        return vec![ModeStep::Refuse];
    }
    let mut steps = Vec::new();
    // `StartWatcher` replaces the watcher it finds, so a plan that ends in one
    // needs no stop of its own; two stops would mint two generations and leave
    // the new watcher's commands looking stale.
    // Every mode but `no` runs a watcher (`yes` one for the name alone), so any
    // of them leaving for a mode other than `auto` stops it.
    if prev.wants_tailscale() && !matches!(next, TailscaleMode::Auto) {
        steps.push(ModeStep::StopWatcher);
    }
    match next {
        TailscaleMode::No => {
            if let Some(addr) = bound {
                steps.push(ModeStep::Unbind(addr));
            }
            steps.push(ModeStep::SetHostLiterals(false));
            steps.push(ModeStep::ForgetIdentity);
        }
        TailscaleMode::Yes => {
            steps.push(ModeStep::SetHostLiterals(true));
            steps.push(ModeStep::DetectAndBind);
        }
        TailscaleMode::Auto => {
            steps.push(ModeStep::SetHostLiterals(true));
            steps.push(ModeStep::StartWatcher { probe_now: true });
        }
    }
    steps
}

/// Everything the watch loop is handed: where the leg would listen, how often to
/// look, the two probes, the two answers the serve loop currently holds, and the
/// two ways back to it.
///
/// Every collaborator is injected so the whole loop is testable with no Tailscale
/// binary, no sockets and no clock: `detect` is the address probe and `identify`
/// the name probe, `bound` and `known_identity` report what the serve loop holds
/// (so a FAILED bind is retried next period rather than being lost, and an
/// identity is only sent when it differs), `emit` hands a command to the serve
/// loop and returns false when nobody is listening any more, and `stop` ends the
/// loop.
pub(crate) struct Watch<'a> {
    pub(crate) primary: SocketAddr,
    pub(crate) period: Duration,
    /// Whether the loop starts by probing or by SLEEPING. A watcher started at
    /// serve time sleeps: the startup bind answered the same question a moment
    /// ago. A watcher started by a live switch to `auto` probes, because the
    /// gesture that started it needs an outcome to report.
    pub(crate) probe_first: bool,
    /// Whether this watcher looks for the ADDRESS at all. `auto` does; `yes`
    /// looked once and does not, but it still watches the NAME for as long as
    /// the Host guard may admit it, because a Funnel switched on later has to
    /// be noticed whatever the address rule says.
    pub(crate) addresses: bool,
    pub(crate) detect: &'a dyn Fn() -> Result<IpAddr, TailscaleUnavailable>,
    pub(crate) identify: &'a dyn Fn() -> Result<TailscaleIdentity, TailscaleUnavailable>,
    pub(crate) bound: &'a dyn Fn() -> Option<SocketAddr>,
    pub(crate) known_identity: &'a dyn Fn() -> Option<TailscaleIdentity>,
    pub(crate) emit: &'a dyn Fn(WatchEvent) -> bool,
    pub(crate) stop: &'a dyn Fn() -> bool,
}

/// Run the watch loop: poll the detectors, compare against what the serve loop
/// holds, emit at most one transition per period, and stop when `stop` says
/// serving is over.
///
/// An identity the serve loop does not have yet is looked up BEFORE the first
/// park even on a watcher that otherwise sleeps first: the address was answered
/// by the startup bind, the name was not, and a browser on the MagicDNS name
/// must not be refused for a whole period because of it.
///
/// `stop` is consulted again after each probe and before its emit: a detection
/// can take seconds, and a watcher stopped during one must not hand the serve
/// loop a command for the mode it just left.
pub(crate) fn watch_tailscale(w: &Watch<'_>) {
    let mut immediate = w.probe_first;
    if !immediate && (w.known_identity)().is_none() && !identity_step(w) {
        return;
    }
    loop {
        if immediate {
            immediate = false;
            if (w.stop)() {
                return;
            }
        } else if !park(w.period, w.stop) {
            return;
        }
        if !w.addresses {
            if !identity_step(w) {
                return;
            }
            continue;
        }
        let desired = desired_leg(w.primary, (w.detect)());
        let step = plan_leg_step((w.bound)(), desired);
        if (w.stop)() {
            return;
        }
        let sent = match step {
            LegStep::Nothing => true,
            LegStep::Bind(addr) => (w.emit)(WatchEvent::Leg(LegCommand::Bind(addr))),
            LegStep::Unbind(addr) => (w.emit)(WatchEvent::Leg(LegCommand::Unbind(addr))),
            LegStep::Rebind { old, new } => {
                (w.emit)(WatchEvent::Leg(LegCommand::Unbind(old)))
                    && (w.emit)(WatchEvent::Leg(LegCommand::Bind(new)))
            }
        };
        if !sent || !identity_step(w) {
            // Stopped, or the serve loop is gone; there is nobody left to tell.
            return;
        }
    }
}

/// One look at the identity. Answers false when the watcher should end.
///
/// A look that failed after one had succeeded is reported as
/// [`WatchEvent::IdentityFailed`], so the name stops being admitted until a look
/// succeeds again. Failing closed costs a tailnet browser the name for one
/// period when the daemon hiccups; failing open would keep admitting a name
/// whose Funnel state nobody could read.
fn identity_step(w: &Watch<'_>) -> bool {
    let identity = match (w.identify)() {
        Ok(identity) => identity,
        Err(reason) => {
            if (w.stop)() {
                return false;
            }
            return (w.emit)(WatchEvent::IdentityFailed(reason));
        }
    };
    if (w.stop)() {
        return false;
    }
    if (w.known_identity)().as_ref() == Some(&identity) {
        return true;
    }
    (w.emit)(WatchEvent::Identity(identity))
}

/// Sleep for `period` in slices, returning false as soon as `stop` says to end.
/// Slicing is what makes a multi-second period compatible with a prompt teardown.
fn park(period: Duration, stop: &dyn Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + period;
    loop {
        if stop() {
            return false;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return !stop();
        }
        std::thread::sleep(remaining.min(WATCH_SLICE));
    }
}

// ── What the identity means ─────────────────────────────────────────────

/// The keyed-status key news about this machine's tailnet NAME is reported
/// under: a rename, a `tailscale serve` route arriving or leaving, Funnel.
///
/// Its own key rather than [`TAILSCALE_LEG_KEY`], because the leg key is
/// coalesced against a flapping interface and a rename is not a flap.
pub(crate) const TAILSCALE_IDENTITY_KEY: &str = "tailscale-identity";

/// The name the Host guard admits for this identity: this machine's MagicDNS
/// name, only when Tailscale assigned it (under the tailnet's `.ts.net`
/// suffix), and never while Funnel is switched on for ANYTHING on this machine.
///
/// Funnel withdraws the name so dux does not offer one a Funnel is using. That
/// is not what keeps a Funnel out: Tailscale's serve proxy forwards the public
/// client's own Host, so a request through Funnel can claim `localhost` and
/// never needs this name. The Funnel marker and the Funnel lockout in the Host
/// guard are the protections (see `host_guard`). The check is any Funnel at
/// all, whatever it forwards to. A name from another control server sits in a
/// domain its operator chose, so it goes through `allowed_hosts` instead.
pub(crate) fn admitted_own_name(identity: &TailscaleIdentity) -> Option<String> {
    if identity.funnel {
        return None;
    }
    identity
        .status
        .tailscale_assigned_name()
        .map(str::to_string)
}

/// The tailnet addresses dux can be opened at, beyond the listener addresses
/// themselves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TailnetUrls {
    /// `http://<tailscale ip>:<port>`, when dux is listening on a Tailscale
    /// address.
    pub(crate) ip: Option<String>,
    /// `http://<magicdns name>:<port>`, when the name resolves to an address dux
    /// is listening on.
    pub(crate) name: Option<String>,
    /// The `tailscale serve` URLs that end at dux.
    pub(crate) serve: Vec<String>,
}

impl TailnetUrls {
    /// The MagicDNS URL worth handing a phone: the HTTPS serve route when there
    /// is one, otherwise the plain name.
    pub(crate) fn magic_dns(&self) -> Option<&str> {
        self.serve
            .first()
            .map(String::as_str)
            .or(self.name.as_deref())
    }

    /// The URLs to show beside the listener addresses: the plain name, then the
    /// serve routes.
    pub(crate) fn extra(&self) -> Vec<String> {
        self.name.iter().chain(self.serve.iter()).cloned().collect()
    }
}

/// Work out the tailnet URLs from the legs dux is serving right now, the
/// identity the watcher last read, and whether the mode wants Tailscale at all.
///
/// - `ip`: the first leg on a Tailscale address.
/// - `name`: the MagicDNS name, when the mode wants Tailscale, MagicDNS is on,
///   the Host guard admits the name, and some leg listens where the name
///   resolves (a Tailscale address, or every address).
/// - `serve`: the serve routes, under the same mode and guard conditions but
///   regardless of the legs, because `tailscale serve` proxies to loopback.
pub(crate) fn tailnet_urls(
    legs: &[SocketAddr],
    identity: Option<&TailscaleIdentity>,
    tailscale_wanted: bool,
) -> TailnetUrls {
    let on_tailscale = |addr: &&SocketAddr| match addr.ip() {
        IpAddr::V4(v4) => dux_core::tailscale::is_tailscale_cgnat(v4),
        IpAddr::V6(v6) => dux_core::tailscale::is_tailscale_ipv6(v6),
    };
    let ip = legs
        .iter()
        .find(on_tailscale)
        .map(|addr| format!("http://{addr}"));
    let admitted = identity
        .filter(|_| tailscale_wanted)
        .and_then(|identity| admitted_own_name(identity).map(|name| (identity, name)));
    let Some((identity, name)) = admitted else {
        return TailnetUrls {
            ip,
            ..TailnetUrls::default()
        };
    };
    let name = legs
        .iter()
        .find(|addr| addr.ip().is_unspecified() || on_tailscale(addr))
        .filter(|_| identity.status.magic_dns_enabled)
        .map(|addr| format!("http://{name}:{}", addr.port()));
    TailnetUrls {
        ip,
        name,
        serve: identity
            .serve
            .iter()
            .map(|route| route.url.clone())
            .collect(),
    }
}

/// The one-time tip that `tailscale serve` would give dux an HTTPS address,
/// when MagicDNS is on and no serve route ends at dux yet.
///
/// The tip names the exact command and says that dux never runs it: serving is
/// a persistent change to the machine, and the choice belongs to whoever owns
/// it. Where HTTPS certificates are off for the tailnet, the tip says to switch
/// them on first, because the command fails without them.
pub(crate) fn serve_hint(identity: &TailscaleIdentity, port: u16) -> Option<String> {
    if !identity.status.magic_dns_enabled || !identity.serve.is_empty() || identity.funnel {
        return None;
    }
    let name = identity.status.dns_name.as_deref()?;
    let certificates = if identity.status.cert_domains.is_empty() {
        " HTTPS certificates are off for this tailnet, so switch them on in the Tailscale \
         admin console (DNS, HTTPS Certificates) first."
    } else {
        ""
    };
    Some(format!(
        "Tip: `tailscale serve --bg {port}` would also put dux at https://{name} with a real \
         certificate, for every device on your tailnet. dux never runs it for you.{certificates}"
    ))
}

/// The sentences one identity change is worth, in order, each with its tone.
///
/// A first look says nothing but a Funnel warning: whatever the first look
/// found is what dux starts with, and the startup banner (or the URL list) is
/// where that is said. After that: a rename, each serve route that arrived or
/// left, and Funnel switching on or off.
pub(crate) fn identity_news(
    previous: Option<&TailscaleIdentity>,
    next: &TailscaleIdentity,
) -> Vec<(dux_core::statusline::StatusTone, String)> {
    use dux_core::statusline::StatusTone;
    let mut news = Vec::new();
    // A Funnel that forwards to dux's port is the lockout's news (see
    // `lockout_news`), said once there; this only speaks for a Funnel elsewhere,
    // which withdraws the name and nothing more.
    let name_funnel = |id: &TailscaleIdentity| id.funnel && !id.funnel_to_dux;
    let funnel_before = previous.is_some_and(name_funnel);
    let funnel_now = name_funnel(next);
    let admitted_before = previous.and_then(admitted_own_name);
    let admitted_now = admitted_own_name(next);
    if funnel_now && !funnel_before {
        let name = next
            .status
            .tailscale_assigned_name()
            .unwrap_or("this machine's tailnet name");
        news.push((
            StatusTone::Warning,
            format!(
                "A Tailscale Funnel route on this machine publishes it to the public internet, \
                 and dux has no login, so dux stopped answering to {name}. That Funnel does \
                 not forward to dux's port, but dux cannot tell whether it reaches dux through \
                 another program on this machine; if it does, anyone on the internet can drive your \
                 terminals. Turn the Funnel route off (`tailscale funnel status` lists it) and \
                 dux answers to {name} again by itself."
            ),
        ));
    }
    let Some(previous) = previous else {
        return news;
    };
    if funnel_before
        && !next.funnel
        && let Some(name) = admitted_now.as_deref()
    {
        news.push((
            StatusTone::Info,
            format!("The Tailscale Funnel route is off, so dux answers to {name} again."),
        ));
    }

    // Said from what the Host guard ADMITS, not from what Tailscale reports: a
    // name dux never answered to (a Headscale domain, a name under Funnel) is
    // nobody's news.
    match (admitted_before.as_deref(), admitted_now.as_deref()) {
        (Some(old), Some(new)) if old != new => news.push((
            StatusTone::Info,
            format!(
                "This machine's tailnet name changed from {old} to {new} (the tailnet or the \
                 machine was renamed). dux now answers to {new} and no longer to {old}."
            ),
        )),
        (Some(old), None) if !next.funnel => news.push((
            StatusTone::Info,
            format!(
                "Tailscale no longer reports {old} as this machine's name (logged out, renamed \
                 outside the tailnet's domain, or MagicDNS off), so dux stopped answering to it."
            ),
        )),
        _ => {}
    }
    if !next.funnel {
        // Measured against EVERY route the previous look had, Funnel or not: a
        // route that only stopped being published is not a new one, and the
        // Funnel sentence above already said what changed about it.
        let before: Vec<&str> = served_urls(previous);
        let before_any: Vec<&str> = previous.serve.iter().map(|r| r.url.as_str()).collect();
        let after: Vec<&str> = served_urls(next);
        for url in after.iter().filter(|url| !before_any.contains(url)) {
            news.push((
                StatusTone::Info,
                format!(
                    "tailscale serve now forwards {url} to dux: open it from any device on your \
                     tailnet. The first visit can take about 30 seconds while Tailscale issues \
                     the certificate."
                ),
            ));
        }
        for url in before.iter().filter(|url| !after.contains(url)) {
            news.push((
                StatusTone::Info,
                format!(
                    "{url} no longer reaches dux: its tailscale serve route was removed or now \
                     points somewhere else."
                ),
            ));
        }
    }
    news
}

/// Why the lockout moved, for the one case where that changes the sentence: a
/// lift to open after a look that SAW no Funnel, against one made because
/// Tailscale is not running here at all, or not installed (nothing was
/// confirmed then).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Because {
    Look,
    NoTailscaleHere,
}

/// Said once at start when dux runs inside a container and its checks find no
/// Tailscale there. dux serves: a Tailscale outside the container is invisible
/// from in here, and publishing a containerised dux through one is a setup the
/// operator chose, so this says what dux cannot know and what that means rather
/// than refusing.
pub(crate) const CONTAINER_WARNING: &str = "dux is running inside a container and sees no \
     Tailscale here. It cannot see a Tailscale outside this container, so it cannot tell \
     whether something outside publishes this port, and dux has no login: anything that can \
     reach this port can drive your terminals. Keep the port private to your own network; dux \
     is not meant to be exposed publicly.";

/// Where a FAILED look leaves the lockout.
///
/// - A Funnel already seen stays refused through anything that is not a look
///   that SEES it gone: a daemon outage, a CLI that fails. Only a successful
///   look lifts it.
/// - An unknown answer (the CLI failed, timed out, could not reach a daemon, or
///   is missing while Tailscale is evidently here) refuses everything, from
///   open too, not only before the first look.
/// - An answer: no Tailscale here at all, or a daemon the CLI says is not
///   running, with nothing else of Tailscale on the machine (the probe reports
///   these only then). Nothing can publish dux, so it serves.
pub(crate) fn lockout_after_failure(
    current: crate::host_guard::FunnelLockout,
    reason: &TailscaleUnavailable,
) -> crate::host_guard::FunnelLockout {
    use crate::host_guard::FunnelLockout::{CliNotFound, Funnel, FunnelSaved, Open, Unconfirmed};
    if matches!(current, Funnel | FunnelSaved) {
        return current;
    }
    match reason {
        TailscaleUnavailable::CommandMissing | TailscaleUnavailable::DaemonStopped => Open,
        TailscaleUnavailable::Unverifiable => CliNotFound,
        TailscaleUnavailable::CommandFailed
        | TailscaleUnavailable::NoAddress
        | TailscaleUnavailable::DaemonUnreachable => Unconfirmed,
    }
}

/// Where a SUCCESSFUL look leaves the lockout: refused while a Funnel publishes
/// dux, with its own way out when the node is down and the Funnel only saved.
pub(crate) fn lockout_after_look(identity: &TailscaleIdentity) -> crate::host_guard::FunnelLockout {
    use crate::host_guard::FunnelLockout::{Funnel, FunnelSaved, Open};
    match (identity.funnel_to_dux, identity.node_down) {
        (true, true) => FunnelSaved,
        (true, false) => Funnel,
        (false, _) => Open,
    }
}

/// What one change of the Funnel lockout is worth saying, if anything. Quiet
/// when the first look finds nothing (the expected outcome) or nothing changed.
pub(crate) fn lockout_news(
    before: crate::host_guard::FunnelLockout,
    after: crate::host_guard::FunnelLockout,
    because: Because,
) -> Option<(dux_core::statusline::StatusTone, String)> {
    use crate::host_guard::FunnelLockout::{
        Checking, CliNotFound, Funnel, FunnelSaved, Open, Unconfirmed,
    };
    use dux_core::statusline::StatusTone;
    if before == after {
        return None;
    }
    match (before, after) {
        (_, Funnel) => Some((
            StatusTone::Warning,
            "A Tailscale Funnel forwards connections from the public internet to dux's port, \
             and dux has no login, so dux is refusing every request until that Funnel is \
             turned off (`tailscale funnel status` lists it)."
                .to_string(),
        )),
        (_, Unconfirmed) => Some((
            StatusTone::Warning,
            "dux could not confirm that no Tailscale Funnel publishes it to the public internet \
             (the tailscale CLI failed, did not answer, or cannot reach its daemon), and dux has \
             no login, so it refuses every request until it can. Fix tailscaled on this machine first (`tailscale status` shows what it says). As a last resort, [server] tailscale = \"no\" stops dux consulting Tailscale, which also turns off this Funnel protection."
                .to_string(),
        )),
        (_, FunnelSaved) => Some((
            StatusTone::Warning,
            crate::host_guard::FUNNEL_SAVED_REFUSAL.to_string(),
        )),
        (_, CliNotFound) => Some((
            StatusTone::Warning,
            crate::host_guard::CLI_NOT_FOUND_REFUSAL.to_string(),
        )),
        (Funnel | FunnelSaved | Unconfirmed | CliNotFound, Open) => Some((
            StatusTone::Info,
            match because {
                Because::Look => "dux confirmed that no Tailscale Funnel publishes it, and \
                                  serves requests again."
                    .to_string(),
                Because::NoTailscaleHere => "Tailscale is not running on this machine (or \
                                             not installed), so nothing can publish dux \
                                             through Funnel; dux serves requests again."
                    .to_string(),
            },
        )),
        (Checking, Open)
        | (Open | Checking | Unconfirmed | CliNotFound | Funnel | FunnelSaved, Checking) => None,
        (Open, Open) => None,
    }
}

/// What a serve that does not consult Tailscale says once at start: what that
/// choice costs, because the Funnel checks are what it turns off.
pub(crate) fn not_checking_tailscale(forced_no: bool) -> String {
    let why = if forced_no {
        "--no-tailscale"
    } else {
        "[server] tailscale = \"no\""
    };
    format!(
        "dux is not checking Tailscale ({why}), so it will not notice a Tailscale Funnel \
         publishing it to the public internet, and dux has no login. Keep this port private to \
         your own network."
    )
}

/// What dux says when a switch to `tailscale = "no"` lifts a refusal: an
/// explicit choice, but one that serves whatever a Funnel publishes.
pub(crate) fn lockout_lifted_by_no() -> String {
    "[server] tailscale is now \"no\", so dux no longer checks for Tailscale Funnel and serves \
     every request again, including any a Funnel publishes from the public internet with no \
     login. Turn the Funnel off, or set tailscale back to \"auto\"."
        .to_string()
}

/// The URLs of the serve routes to dux that stay on the tailnet.
fn served_urls(identity: &TailscaleIdentity) -> Vec<&str> {
    identity
        .serve
        .iter()
        .filter(|route| !route.funnel)
        .map(|route| route.url.as_str())
        .collect()
}

// ── The live mode lane ───────────────────────────────────────────

/// Depth of the mode-request lane. A person changing a tri-state, so anything
/// above a couple of slots is theatre; the bound keeps a wedged serve loop from
/// letting a caller grow memory.
const MODE_REQUEST_QUEUE: usize = 8;

/// How long a caller waits for the serve loop to answer a mode change.
///
/// Slightly above [`dux_core::tailscale::DETECT_TIMEOUT`], because a `yes` runs
/// one bounded detection before it can say anything. No surface may wait longer
/// than this: the TUI resolves a status op on the answer and a browser holds a
/// request open for it.
const MODE_REQUEST_TIMEOUT: Duration = Duration::from_secs(7);

/// One live mode change on its way to the serve loop, with the lane its answer
/// comes back on.
pub(crate) struct ModeRequest {
    pub(crate) mode: TailscaleMode,
    pub(crate) reply: tokio::sync::oneshot::Sender<TailscaleModeOutcome>,
}

/// The one handle every surface changes `[server] tailscale` through while dux
/// is serving: the `dux server` route, the flip's route, and the terminal UI's
/// companion in background mode.
///
/// A request lane rather than a shared value, because a mode change is an ACT
/// (stop a watcher, drop a listener, run a bounded detection) and only the serve
/// loop may perform it. Every request is answered: one superseded by a later one
/// resolves [`TailscaleModeOutcome::Superseded`] rather than being dropped
/// silently, and a lane whose loop has gone resolves
/// [`TailscaleModeOutcome::NotServing`].
#[derive(Clone)]
pub struct TailscaleModeControl {
    tx: tokio::sync::mpsc::Sender<ModeRequest>,
    /// The serve's runtime, so a caller on a thread with no runtime of its own
    /// (the engine actor's reload arm, the terminal UI's run loop) can still ask
    /// for a mode change and hear the answer.
    runtime: tokio::runtime::Handle,
    /// Whether an interface watcher is running for this serve right now. Shared
    /// with [`ServeShutdown`] so a dying best-effort leg's warning stays truthful
    /// after a live switch between `auto` and the static modes.
    watched: Arc<AtomicBool>,
    /// Whether the Host guard accepts Tailscale IP literals. The serve loop is
    /// its only writer; the guard reads it per request.
    host_literals: Arc<AtomicBool>,
    /// The identity the Tailscale watcher last read. The serve loop is its only
    /// writer; the URL lists every surface shows read it.
    identity: IdentityCell,
    /// The MagicDNS name the Host guard admits (rule 6). The serve loop is its
    /// only writer; the guard reads it per request.
    own_name: crate::host_guard::LiveHostNames,
    /// Whether the Host guard serves at all, as far as Funnel goes. Starts
    /// CHECKING on every mode but `no`, so nothing is served before the first
    /// look at Tailscale lands. The serve loop is its only writer.
    funnel_lockout: crate::host_guard::FunnelLockoutCell,
}

/// The identity a serve currently holds, shared between the serve loop that
/// writes it and the surfaces that read the URLs it implies.
pub(crate) type IdentityCell = Arc<std::sync::Mutex<Option<TailscaleIdentity>>>;

impl TailscaleModeControl {
    /// Build the control and the receiving end the serve loop owns.
    pub(crate) fn new(
        runtime: tokio::runtime::Handle,
        watched: Arc<AtomicBool>,
        host_literals: Arc<AtomicBool>,
    ) -> (Self, tokio::sync::mpsc::Receiver<ModeRequest>) {
        let (tx, rx) = tokio::sync::mpsc::channel(MODE_REQUEST_QUEUE);
        let funnel_lockout =
            crate::host_guard::FunnelLockoutCell::new(if host_literals.load(Ordering::SeqCst) {
                crate::host_guard::FunnelLockout::Checking
            } else {
                crate::host_guard::FunnelLockout::Open
            });
        (
            Self {
                tx,
                runtime,
                watched,
                host_literals,
                identity: IdentityCell::default(),
                own_name: crate::host_guard::LiveHostNames::default(),
                funnel_lockout,
            },
            rx,
        )
    }

    /// The cell the Host guard reads rule 5 from.
    pub fn host_literals(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.host_literals)
    }

    /// The set the Host guard reads rule 6 (this machine's own MagicDNS name)
    /// from.
    pub fn own_magicdns_name(&self) -> crate::host_guard::LiveHostNames {
        self.own_name.clone()
    }

    /// The cell the Host guard reads to refuse every request while a Tailscale
    /// Funnel forwards raw TCP to dux's port.
    pub fn funnel_lockout(&self) -> crate::host_guard::FunnelLockoutCell {
        self.funnel_lockout.clone()
    }

    /// The identity the serve loop holds, for the URL lists.
    pub(crate) fn identity(&self) -> IdentityCell {
        Arc::clone(&self.identity)
    }

    /// The tailnet URLs this serve can be opened at right now, beyond the
    /// listener addresses, given the legs it is serving.
    pub(crate) fn tailnet_urls(&self, legs: &[SocketAddr]) -> TailnetUrls {
        let identity = self
            .identity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        tailnet_urls(
            legs,
            identity.as_ref(),
            self.host_literals.load(Ordering::SeqCst),
        )
    }

    /// The cell [`ServeShutdown`] reads to say whether anything will bind the
    /// Tailscale leg again.
    pub(crate) fn watched(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.watched)
    }

    /// Ask for a mode change from a thread with no runtime and report what
    /// happened when the loop answers.
    ///
    /// The synchronous surfaces need this: the engine actor's reload arm and the
    /// terminal UI's run loop both sit on plain std threads, and neither may
    /// block on a detection that is allowed to take five seconds.
    pub fn set_mode_detached(
        &self,
        mode: TailscaleMode,
        report: impl FnOnce(TailscaleModeOutcome) + Send + 'static,
    ) {
        let control = self.clone();
        self.runtime.spawn(async move {
            report(control.set_mode(mode).await);
        });
    }

    /// Ask the serve loop to change the mode and wait for what it did.
    ///
    /// Bounded on purpose: a `yes` runs a detection that is itself bounded, and
    /// a caller that waited forever would be a hung status op or a hung HTTP
    /// request.
    pub async fn set_mode(&self, mode: TailscaleMode) -> TailscaleModeOutcome {
        let (reply, answer) = tokio::sync::oneshot::channel();
        if self.tx.send(ModeRequest { mode, reply }).await.is_err() {
            return TailscaleModeOutcome::NotServing;
        }
        match tokio::time::timeout(MODE_REQUEST_TIMEOUT, answer).await {
            Ok(Ok(outcome)) => outcome,
            // The loop dropped the reply lane, which only happens as a serve
            // ends.
            Ok(Err(_)) => TailscaleModeOutcome::NotServing,
            Err(_) => TailscaleModeOutcome::TimedOut,
        }
    }
}

/// What became of the Tailscale leg during startup. The banner's note has to tell
/// these apart: "there is no address yet" and "the address is right there but
/// would not bind" are different situations, and telling the second operator that
/// dux is waiting for an interface they can plainly see is up reads as a bug in
/// dux rather than as the port conflict it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupLeg {
    /// An address was detected and its listener bound. Nothing to say.
    Bound,
    /// An address was detected, but binding it failed (something else holds that
    /// port). The bind failure itself is already a warning row; this is about what
    /// happens next.
    BindFailed,
    /// No Tailscale address was detected at all.
    Undetected,
}

/// The banner / status note for a serve on `auto` whose Tailscale leg is not
/// serving. Returns `None` when there is nothing to add: the leg is up, or the
/// mode is a static answer and the bind warnings already say all there is to say.
///
/// This is the third state the surfacing story learns: not "Tailscale is off" and
/// not "Tailscale is bound", but "not yet, and dux is watching".
pub(crate) fn waiting_note(mode: TailscaleMode, leg: StartupLeg) -> Option<String> {
    if !mode.watches_interface() {
        return None;
    }
    match leg {
        StartupLeg::Bound => None,
        StartupLeg::Undetected => Some(
            "Tailscale: waiting for the interface (auto). dux is serving without it and will \
             bind your Tailscale address by itself when it appears."
                .to_string(),
        ),
        StartupLeg::BindFailed => Some(format!(
            "Tailscale: the interface is here, but its address would not bind (see the warning \
             above and dux.log). dux is serving without it and tries again about every {}s \
             (auto), so freeing that port is enough.",
            WATCH_PERIOD.as_secs()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    /// The watch loop with the identity probe answering nothing, for the tests
    /// that are about the address leg alone.
    fn watch_legs_only(
        primary: SocketAddr,
        period: Duration,
        probe_first: bool,
        detect: &dyn Fn() -> Result<IpAddr, TailscaleUnavailable>,
        bound: &dyn Fn() -> Option<SocketAddr>,
        emit: &dyn Fn(LegCommand) -> bool,
        stop: &dyn Fn() -> bool,
    ) {
        watch_tailscale(&Watch {
            primary,
            period,
            probe_first,
            addresses: true,
            detect,
            identify: &|| Err(TailscaleUnavailable::CommandMissing),
            bound,
            known_identity: &|| None,
            emit: &|event| match event {
                WatchEvent::Leg(command) => emit(command),
                WatchEvent::Identity(_) | WatchEvent::IdentityFailed(_) => true,
            },
            stop,
        });
    }

    // ── The pure step decision ────────────────────────────────────────────

    #[test]
    fn a_leg_is_bound_when_the_interface_appears_and_dropped_when_it_goes() {
        let ts = addr("100.64.0.5:8080");
        assert_eq!(plan_leg_step(None, Some(ts)), LegStep::Bind(ts));
        assert_eq!(plan_leg_step(Some(ts), None), LegStep::Unbind(ts));
        assert_eq!(plan_leg_step(None, None), LegStep::Nothing);
        assert_eq!(plan_leg_step(Some(ts), Some(ts)), LegStep::Nothing);
    }

    #[test]
    fn a_changed_tailscale_address_rebinds_rather_than_stacking_listeners() {
        let old = addr("100.64.0.5:8080");
        let new = addr("100.64.0.9:8080");
        assert_eq!(
            plan_leg_step(Some(old), Some(new)),
            LegStep::Rebind { old, new }
        );
    }

    #[test]
    fn a_primary_that_already_covers_tailscale_never_grows_a_leg() {
        let ip: IpAddr = "100.64.0.5".parse().unwrap();
        // A wildcard primary is already listening on the Tailscale interface.
        assert_eq!(desired_leg(addr("0.0.0.0:8080"), Ok(ip)), None);
        assert_eq!(desired_leg(addr("[::]:8080"), Ok(ip)), None);
        // And a primary bound to the Tailscale address itself IS the leg.
        assert_eq!(desired_leg(addr("100.64.0.5:8080"), Ok(ip)), None);
        // An ordinary loopback primary does want the leg, at the same port.
        assert_eq!(
            desired_leg(addr("127.0.0.1:9000"), Ok(ip)),
            Some(addr("100.64.0.5:9000"))
        );
    }

    #[test]
    fn an_undetectable_address_wants_no_leg_whatever_the_reason() {
        for reason in [
            TailscaleUnavailable::CommandMissing,
            TailscaleUnavailable::CommandFailed,
            TailscaleUnavailable::NoAddress,
        ] {
            assert_eq!(desired_leg(addr("127.0.0.1:8080"), Err(reason)), None);
        }
    }

    // ── The watch loop, with every collaborator faked ─────────────────────

    /// A scripted detector plus the serve loop's bound state, driving the real
    /// watch loop with no Tailscale binary, no sockets and no waiting.
    struct Harness {
        script: Mutex<Vec<Result<IpAddr, TailscaleUnavailable>>>,
        /// How many periods the script covers. The stop closure lags one probe
        /// behind it, because the loop re-checks `stop` AFTER the probe: a stop
        /// that fired on the last scripted probe would swallow that period's
        /// command and every script would be one transition short.
        periods: usize,
        probes: Mutex<usize>,
        bound: Mutex<Option<SocketAddr>>,
        /// When set, a Bind command is NOT reflected into `bound`, standing in for
        /// a best-effort bind that failed.
        refuse_binds: bool,
        sent: Mutex<Vec<LegCommand>>,
    }

    impl Harness {
        fn new(script: Vec<Result<IpAddr, TailscaleUnavailable>>) -> Self {
            Self {
                periods: script.len(),
                script: Mutex::new(script),
                probes: Mutex::new(0),
                bound: Mutex::new(None),
                refuse_binds: false,
                sent: Mutex::new(Vec::new()),
            }
        }

        fn run(&self, primary: SocketAddr) -> Vec<LegCommand> {
            watch_legs_only(
                primary,
                Duration::ZERO,
                false,
                &|| {
                    *self.probes.lock().unwrap() += 1;
                    let mut script = self.script.lock().unwrap();
                    if script.is_empty() {
                        // Exhausted: the stop closure below ends the loop on this
                        // probe, so this is never consulted for a decision.
                        return Err(TailscaleUnavailable::NoAddress);
                    }
                    script.remove(0)
                },
                &|| *self.bound.lock().unwrap(),
                &|cmd| {
                    self.sent.lock().unwrap().push(cmd);
                    match cmd {
                        LegCommand::Bind(a) if !self.refuse_binds => {
                            *self.bound.lock().unwrap() = Some(a);
                        }
                        LegCommand::Bind(_) => {}
                        LegCommand::Unbind(_) => *self.bound.lock().unwrap() = None,
                    }
                    true
                },
                &|| *self.probes.lock().unwrap() > self.periods,
            );
            self.sent.lock().unwrap().clone()
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_watcher_binds_when_the_interface_appears() {
        // Absent, absent, then present: exactly one Bind, on the period that saw
        // it appear.
        let h = Harness::new(vec![
            Err(TailscaleUnavailable::CommandFailed),
            Err(TailscaleUnavailable::CommandFailed),
            Ok(ip("100.64.0.5")),
        ]);
        assert_eq!(
            h.run(addr("127.0.0.1:8080")),
            vec![LegCommand::Bind(addr("100.64.0.5:8080"))]
        );
    }

    #[test]
    fn the_watcher_unbinds_when_the_interface_goes_away() {
        let h = Harness::new(vec![
            Ok(ip("100.64.0.5")),
            Ok(ip("100.64.0.5")),
            Err(TailscaleUnavailable::CommandFailed),
        ]);
        assert_eq!(
            h.run(addr("127.0.0.1:8080")),
            vec![
                LegCommand::Bind(addr("100.64.0.5:8080")),
                LegCommand::Unbind(addr("100.64.0.5:8080")),
            ]
        );
    }

    #[test]
    fn a_steady_interface_produces_no_commands_at_all() {
        // The common case must be silent: no churn, no log spam, no rebinding a
        // listener that is fine.
        let h = Harness::new(vec![Ok(ip("100.64.0.5")); 5]);
        assert_eq!(
            h.run(addr("127.0.0.1:8080")),
            vec![LegCommand::Bind(addr("100.64.0.5:8080"))],
            "one bind on the first period, then silence"
        );
    }

    #[test]
    fn a_flap_inside_one_period_costs_at_most_one_transition() {
        // The detect period IS the debounce: the watcher only ever sees the state
        // at the sample, so an interface that came and went between samples
        // produces nothing.
        let h = Harness::new(vec![
            Ok(ip("100.64.0.5")),
            // Away and back between these two samples is invisible by
            // construction; the sample says present, and nothing is emitted.
            Ok(ip("100.64.0.5")),
        ]);
        assert_eq!(h.run(addr("127.0.0.1:8080")).len(), 1);
    }

    #[test]
    fn a_failed_bind_is_retried_on_the_next_period() {
        // The watcher compares against what is actually BOUND, not against what
        // it last asked for, so a best-effort bind that failed (a busy port, a
        // half-configured interface) is asked for again rather than lost until
        // the next flap.
        let mut h = Harness::new(vec![Ok(ip("100.64.0.5")), Ok(ip("100.64.0.5"))]);
        h.refuse_binds = true;
        assert_eq!(
            h.run(addr("127.0.0.1:8080")),
            vec![
                LegCommand::Bind(addr("100.64.0.5:8080")),
                LegCommand::Bind(addr("100.64.0.5:8080")),
            ]
        );
    }

    #[test]
    fn the_watcher_stops_when_nobody_is_listening_to_it() {
        // The serve loop has gone (teardown): the watcher must return rather than
        // keep probing a dead server forever.
        let calls = Mutex::new(0usize);
        watch_legs_only(
            addr("127.0.0.1:8080"),
            Duration::ZERO,
            false,
            &|| {
                *calls.lock().unwrap() += 1;
                Ok(ip("100.64.0.5"))
            },
            &|| None,
            &|_| false,
            &|| false,
        );
        assert_eq!(
            *calls.lock().unwrap(),
            1,
            "one probe, one refused send, then the loop ends"
        );
    }

    /// The cadence and the dwell, pinned: dux looks every five seconds and holds
    /// a new state for six before it says anything, so a dwell is always longer
    /// than the period it waits out.
    #[test]
    fn the_watch_period_is_five_seconds_and_the_dwell_outlasts_it() {
        assert_eq!(WATCH_PERIOD, Duration::from_secs(5));
        assert_eq!(LEG_SETTLE_DWELL, Duration::from_secs(6));
        assert!(LEG_SETTLE_DWELL > WATCH_PERIOD);
    }

    /// A probe that outlasts the whole period delays the next look rather than
    /// running beside it: the watcher parks, probes and acts in one sequence on
    /// one thread, so a wedged `tailscale ip` can never produce two at once.
    #[test]
    fn a_probe_slower_than_the_period_never_overlaps_the_next_one() {
        let inside = std::sync::atomic::AtomicBool::new(false);
        let calls = Mutex::new(0usize);
        watch_legs_only(
            addr("127.0.0.1:8080"),
            Duration::from_millis(5),
            true,
            &|| {
                assert!(
                    !inside.swap(true, Ordering::SeqCst),
                    "a second look started while the first was still running"
                );
                std::thread::sleep(Duration::from_millis(25));
                *calls.lock().unwrap() += 1;
                inside.store(false, Ordering::SeqCst);
                Ok(ip("100.64.0.5"))
            },
            &|| None,
            &|_| *calls.lock().unwrap() < 3,
            &|| false,
        );
        assert_eq!(
            *calls.lock().unwrap(),
            3,
            "three looks, one after the other"
        );
    }

    #[test]
    fn the_stop_flag_ends_the_watcher_before_it_probes() {
        let calls = Mutex::new(0usize);
        watch_legs_only(
            addr("127.0.0.1:8080"),
            Duration::ZERO,
            false,
            &|| {
                *calls.lock().unwrap() += 1;
                Ok(ip("100.64.0.5"))
            },
            &|| None,
            &|_| true,
            &|| true,
        );
        assert_eq!(*calls.lock().unwrap(), 0, "a stopped watcher never probes");
    }

    // ── The identity look ─────────────────────────────────────────────────

    fn identity(name: &str, serve: &[(&str, bool)]) -> TailscaleIdentity {
        TailscaleIdentity {
            status: dux_core::tailscale::SelfStatus {
                dns_name: Some(name.to_string()),
                magic_dns_enabled: true,
                magic_dns_suffix: name.split_once('.').map(|(_, s)| s.to_string()),
                cert_domains: vec![name.to_string()],
                tailscale_ips: Vec::new(),
            },
            serve: serve
                .iter()
                .map(|(url, funnel)| dux_core::tailscale::ServeRoute {
                    url: url.to_string(),
                    funnel: *funnel,
                })
                .collect(),
            funnel: serve.iter().any(|(_, funnel)| *funnel),
            funnel_to_dux: false,
            node_down: false,
        }
    }

    /// A machine whose Funnel forwards raw TCP to dux's port.
    fn tcp_funnelled(name: &str) -> TailscaleIdentity {
        TailscaleIdentity {
            funnel: true,
            funnel_to_dux: true,
            ..identity(name, &[])
        }
    }

    fn headscale(host: &str) -> TailscaleIdentity {
        let mut id = identity(&format!("{host}.vpn.example.com"), &[]);
        id.status.magic_dns_suffix = Some("vpn.example.com".to_string());
        id
    }

    /// A machine with Funnel on for something that is NOT a route dux shows:
    /// a TCP forward, another port, a route through the Tailscale address.
    fn funnelled_elsewhere(name: &str) -> TailscaleIdentity {
        TailscaleIdentity {
            funnel: true,
            ..identity(name, &[])
        }
    }

    /// Runs the real watch loop over a scripted identity probe, with the address
    /// probe steady and already bound, and a `known` cell the emit writes the
    /// way the serve loop would.
    fn run_identity_script(
        script: Vec<Result<TailscaleIdentity, TailscaleUnavailable>>,
    ) -> Vec<WatchEvent> {
        let periods = script.len();
        let script = Mutex::new(script);
        let looks = Mutex::new(0usize);
        let known: Mutex<Option<TailscaleIdentity>> = Mutex::new(None);
        let sent = Mutex::new(Vec::new());
        let leg = addr("100.64.0.5:8080");
        watch_tailscale(&Watch {
            primary: addr("127.0.0.1:8080"),
            period: Duration::ZERO,
            probe_first: false,
            addresses: true,
            detect: &|| Ok(ip("100.64.0.5")),
            identify: &|| {
                *looks.lock().unwrap() += 1;
                let mut script = script.lock().unwrap();
                if script.is_empty() {
                    Err(TailscaleUnavailable::CommandFailed)
                } else {
                    script.remove(0)
                }
            },
            bound: &|| Some(leg),
            known_identity: &|| known.lock().unwrap().clone(),
            emit: &|cmd| {
                // What the serve loop does: hold an identity it is sent, and
                // treat it as unknown again once a look failed.
                match &cmd {
                    WatchEvent::Identity(id) => *known.lock().unwrap() = Some(id.clone()),
                    WatchEvent::IdentityFailed(_) => *known.lock().unwrap() = None,
                    WatchEvent::Leg(_) => {}
                }
                sent.lock().unwrap().push(cmd);
                true
            },
            stop: &|| *looks.lock().unwrap() > periods,
        });
        sent.into_inner().unwrap()
    }

    #[test]
    fn a_new_identity_is_sent_once_and_then_the_watcher_is_quiet() {
        let a = identity("box.tail.ts.net", &[]);
        assert_eq!(
            run_identity_script(vec![Ok(a.clone()), Ok(a.clone()), Ok(a.clone())]),
            vec![WatchEvent::Identity(a)]
        );
    }

    #[test]
    fn a_tailnet_rename_is_sent_on_the_look_that_sees_it() {
        let before = identity("demo-box.old-tailnet.ts.net", &[]);
        let after = identity("demo-box.example-tailnet.ts.net", &[]);
        assert_eq!(
            run_identity_script(vec![Ok(before.clone()), Ok(after.clone())]),
            vec![WatchEvent::Identity(before), WatchEvent::Identity(after)]
        );
    }

    #[test]
    fn a_serve_route_arriving_is_an_identity_change_too() {
        let plain = identity("box.tail.ts.net", &[]);
        let served = identity("box.tail.ts.net", &[("https://box.tail.ts.net", false)]);
        assert_eq!(
            run_identity_script(vec![Ok(plain.clone()), Ok(served.clone())]),
            vec![WatchEvent::Identity(plain), WatchEvent::Identity(served)]
        );
    }

    #[test]
    fn an_unknown_identity_is_looked_up_before_the_first_park() {
        // A watcher started with the serve parks before its first ADDRESS look,
        // because the startup bind answered that; the name it has never read.
        // An hour-long period proves the look did not wait for the park.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let sent = Mutex::new(Vec::new());
            let looked = AtomicBool::new(false);
            watch_tailscale(&Watch {
                primary: addr("127.0.0.1:8080"),
                period: Duration::from_secs(3600),
                probe_first: false,
                addresses: true,
                detect: &|| Ok(ip("100.64.0.5")),
                identify: &|| {
                    looked.store(true, Ordering::SeqCst);
                    Ok(identity("box.tail.ts.net", &[]))
                },
                bound: &|| None,
                known_identity: &|| None,
                emit: &|cmd| {
                    sent.lock().unwrap().push(cmd);
                    true
                },
                stop: &|| looked.load(Ordering::SeqCst) && !sent.lock().unwrap().is_empty(),
            });
            let _ = tx.send(sent.into_inner().unwrap());
        });
        let sent = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the identity must be looked up before the hour-long park");
        assert_eq!(
            sent,
            vec![WatchEvent::Identity(identity("box.tail.ts.net", &[]))]
        );
    }

    #[test]
    fn a_watcher_stopped_during_the_identity_look_sends_nothing() {
        let stopped = AtomicBool::new(false);
        let sent = Mutex::new(Vec::new());
        watch_tailscale(&Watch {
            primary: addr("127.0.0.1:8080"),
            period: Duration::ZERO,
            probe_first: false,
            addresses: true,
            detect: &|| Err(TailscaleUnavailable::NoAddress),
            identify: &|| {
                stopped.store(true, Ordering::SeqCst);
                Ok(identity("box.tail.ts.net", &[]))
            },
            bound: &|| None,
            known_identity: &|| None,
            emit: &|cmd| {
                sent.lock().unwrap().push(cmd);
                true
            },
            stop: &|| stopped.load(Ordering::SeqCst),
        });
        assert!(sent.lock().unwrap().is_empty());
    }

    // ── What the identity means ───────────────────────────────────────────

    #[test]
    fn the_guard_admits_the_name_unless_funnel_publishes_a_route_to_dux() {
        assert_eq!(
            admitted_own_name(&identity("box.tail.ts.net", &[])).as_deref(),
            Some("box.tail.ts.net")
        );
        assert_eq!(
            admitted_own_name(&identity(
                "box.tail.ts.net",
                &[("https://box.tail.ts.net", false)]
            ))
            .as_deref(),
            Some("box.tail.ts.net")
        );
        assert_eq!(
            admitted_own_name(&identity(
                "box.tail.ts.net",
                &[("https://box.tail.ts.net", true)]
            )),
            None,
            "a public, login-free dux is never something rule 6 hands out"
        );
        let mut nameless = identity("box.tail.ts.net", &[]);
        nameless.status.dns_name = None;
        assert_eq!(admitted_own_name(&nameless), None);
    }

    #[test]
    fn the_name_url_needs_a_listener_the_name_resolves_to() {
        let id = identity("box.tail.ts.net", &[]);
        let loopback = addr("127.0.0.1:3890");
        let leg = addr("100.64.0.5:3890");

        let urls = tailnet_urls(&[loopback, leg], Some(&id), true);
        assert_eq!(urls.ip.as_deref(), Some("http://100.64.0.5:3890"));
        assert_eq!(urls.name.as_deref(), Some("http://box.tail.ts.net:3890"));
        assert_eq!(urls.magic_dns(), Some("http://box.tail.ts.net:3890"));
        assert_eq!(
            urls.extra(),
            vec!["http://box.tail.ts.net:3890".to_string()]
        );

        // Loopback only (the leg is away): the name resolves to an address
        // nothing is listening on, so it is not offered.
        let urls = tailnet_urls(&[loopback], Some(&id), true);
        assert_eq!(urls, TailnetUrls::default());

        // A wildcard listener covers the Tailscale address too.
        let urls = tailnet_urls(&[addr("0.0.0.0:3890")], Some(&id), true);
        assert_eq!(urls.name.as_deref(), Some("http://box.tail.ts.net:3890"));
        assert_eq!(urls.ip, None, "no Tailscale address is known from the legs");
    }

    #[test]
    fn a_serve_route_is_offered_even_with_the_leg_away_and_wins_the_qr_code() {
        // `tailscale serve` proxies to loopback, so it reaches dux whatever the
        // Tailscale leg is doing.
        let id = identity("box.tail.ts.net", &[("https://box.tail.ts.net", false)]);
        let urls = tailnet_urls(&[addr("127.0.0.1:3890")], Some(&id), true);
        assert_eq!(urls.serve, vec!["https://box.tail.ts.net".to_string()]);
        assert_eq!(urls.magic_dns(), Some("https://box.tail.ts.net"));

        let urls = tailnet_urls(
            &[addr("127.0.0.1:3890"), addr("100.64.0.5:3890")],
            Some(&id),
            true,
        );
        assert_eq!(urls.magic_dns(), Some("https://box.tail.ts.net"));
        assert_eq!(
            urls.extra(),
            vec![
                "http://box.tail.ts.net:3890".to_string(),
                "https://box.tail.ts.net".to_string(),
            ]
        );
    }

    #[test]
    fn no_name_url_is_offered_when_the_mode_is_no_magicdns_is_off_or_funnel_is_on() {
        let legs = [addr("127.0.0.1:3890"), addr("100.64.0.5:3890")];
        let id = identity("box.tail.ts.net", &[("https://box.tail.ts.net", false)]);

        let urls = tailnet_urls(&legs, Some(&id), false);
        assert_eq!((urls.name, urls.serve), (None, vec![]), "mode no");

        let mut off = id.clone();
        off.status.magic_dns_enabled = false;
        let urls = tailnet_urls(&legs, Some(&off), true);
        assert_eq!(
            urls.name, None,
            "the name does not resolve with MagicDNS off"
        );

        let funnelled = identity("box.tail.ts.net", &[("https://box.tail.ts.net", true)]);
        let urls = tailnet_urls(&legs, Some(&funnelled), true);
        assert_eq!(
            (urls.name, urls.serve),
            (None, vec![]),
            "the guard refuses the name, so no URL that uses it is offered"
        );

        let urls = tailnet_urls(&legs, None, true);
        assert_eq!(urls.ip.as_deref(), Some("http://100.64.0.5:3890"));
        assert_eq!(urls.name, None, "nothing read yet");
    }

    #[test]
    fn the_serve_hint_names_the_command_and_only_when_it_would_help() {
        let hint = serve_hint(&identity("box.tail.ts.net", &[]), 3890).expect("no route yet");
        assert!(hint.contains("tailscale serve --bg 3890"), "{hint}");
        assert!(hint.contains("https://box.tail.ts.net"), "{hint}");
        assert_eq!(
            serve_hint(
                &identity("box.tail.ts.net", &[("https://box.tail.ts.net", false)]),
                3890
            ),
            None,
            "a route already ends at dux"
        );
        let mut off = identity("box.tail.ts.net", &[]);
        off.status.magic_dns_enabled = false;
        assert_eq!(serve_hint(&off, 3890), None, "MagicDNS is off");
        let mut no_certs = identity("box.tail.ts.net", &[]);
        no_certs.status.cert_domains.clear();
        let hint = serve_hint(&no_certs, 3890).expect("still worth saying");
        assert!(
            hint.contains("HTTPS certificates"),
            "names what to switch on first: {hint}"
        );
    }

    #[test]
    fn identity_news_says_a_rename_a_route_and_funnel_and_nothing_on_a_first_look() {
        use dux_core::statusline::StatusTone;
        let first = identity("demo-box.old-tailnet.ts.net", &[]);
        assert!(
            identity_news(None, &first).is_empty(),
            "the banner already said it"
        );

        let renamed = identity("demo-box.example-tailnet.ts.net", &[]);
        let news = identity_news(Some(&first), &renamed);
        assert_eq!(news.len(), 1, "{news:?}");
        assert_eq!(news[0].0, StatusTone::Info);
        assert!(
            news[0].1.contains("demo-box.example-tailnet.ts.net"),
            "{news:?}"
        );
        assert!(
            news[0].1.contains("demo-box.old-tailnet.ts.net"),
            "{news:?}"
        );

        let served = identity(
            "demo-box.example-tailnet.ts.net",
            &[("https://demo-box.example-tailnet.ts.net", false)],
        );
        let news = identity_news(Some(&renamed), &served);
        assert_eq!(news.len(), 1, "{news:?}");
        assert!(
            news[0]
                .1
                .contains("https://demo-box.example-tailnet.ts.net"),
            "{news:?}"
        );
        let news = identity_news(Some(&served), &renamed);
        assert_eq!(news.len(), 1, "a route leaving is said too: {news:?}");
        assert!(news[0].1.contains("no longer"), "{news:?}");

        let funnelled = identity(
            "demo-box.example-tailnet.ts.net",
            &[("https://demo-box.example-tailnet.ts.net", true)],
        );
        let news = identity_news(Some(&served), &funnelled);
        assert_eq!(news.len(), 1, "{news:?}");
        assert_eq!(news[0].0, StatusTone::Warning);
        assert!(news[0].1.contains("Funnel"), "{news:?}");
        assert!(news[0].1.contains("no login"), "says why: {news:?}");
        assert!(news[0].1.contains("off"), "says how to stop it: {news:?}");
        assert!(
            !news[0].1.contains("allowed_hosts"),
            "never offers a way to keep Funnel working: {news:?}"
        );
        let news = identity_news(Some(&funnelled), &served);
        assert_eq!(news.len(), 1, "{news:?}");
        assert_eq!(
            news[0].0,
            StatusTone::Info,
            "Funnel going away is good news"
        );
    }

    #[test]
    fn funnel_on_a_route_dux_does_not_show_still_withdraws_the_name_and_warns() {
        let elsewhere = funnelled_elsewhere("box.tail.ts.net");
        assert_eq!(admitted_own_name(&elsewhere), None);
        let news = identity_news(Some(&identity("box.tail.ts.net", &[])), &elsewhere);
        assert_eq!(news.len(), 1, "{news:?}");
        assert_eq!(news[0].0, dux_core::statusline::StatusTone::Warning);
        assert!(!news[0].1.contains("allowed_hosts"), "{news:?}");
        // dux does not lock for it (it may be the operator's own relay), so the
        // warning says plainly what dux cannot know and what that means.
        assert!(
            news[0].1.contains("cannot tell whether") && news[0].1.contains("another program"),
            "{news:?}"
        );
        assert!(news[0].1.contains("no login"), "{news:?}");
        assert_eq!(
            serve_hint(&elsewhere, 3890),
            None,
            "no tip about serving while Funnel is on"
        );
    }

    #[test]
    fn every_change_of_the_lockout_is_said_once_with_why_and_the_way_out() {
        use crate::host_guard::FunnelLockout::{Checking, Funnel, Open, Unconfirmed};
        use dux_core::statusline::StatusTone;
        let look = Because::Look;
        assert_eq!(
            lockout_news(Checking, Open, look),
            None,
            "the expected outcome is quiet"
        );
        assert_eq!(lockout_news(Open, Open, look), None);
        assert_eq!(lockout_news(Funnel, Funnel, look), None);

        for before in [Checking, Open] {
            let (tone, text) = lockout_news(before, Unconfirmed, look).expect("refusing is news");
            assert_eq!(tone, StatusTone::Warning);
            assert!(text.contains("could not confirm"), "{text}");
            let fix = text.find("tailscaled").expect("names tailscaled");
            let no = text.find("tailscale = \"no\"").expect("names the way out");
            assert!(fix < no, "fixing tailscaled comes first: {text}");
            assert!(text.contains("turns off"), "says what `no` costs: {text}");
        }

        for before in [Open, Checking, Unconfirmed] {
            let (tone, text) = lockout_news(before, Funnel, look).expect("a Funnel is news");
            assert_eq!(tone, StatusTone::Warning);
            assert!(text.contains("every request"), "{text}");
            assert!(text.contains("no login"), "{text}");
            assert!(text.contains("tailscale funnel status"), "{text}");
            assert!(!text.contains("allowed_hosts"), "{text}");
        }

        for before in [Funnel, Unconfirmed] {
            let (tone, text) = lockout_news(before, Open, look).expect("lifting is news");
            assert_eq!(tone, StatusTone::Info);
            assert!(text.contains("confirmed"), "a look did confirm it: {text}");
            assert!(text.contains("serves requests again"), "{text}");

            let (_, text) =
                lockout_news(before, Open, Because::NoTailscaleHere).expect("lifting is news");
            assert!(
                !text.contains("confirmed"),
                "nothing was confirmed when there is no Tailscale to ask: {text}"
            );
            assert!(text.contains("not running on this machine"), "{text}");
        }
    }

    #[test]
    fn a_failed_look_never_lifts_a_refusal_and_refuses_while_unknown() {
        use crate::host_guard::FunnelLockout::{Checking, Funnel, Open, Unconfirmed};
        use TailscaleUnavailable::{
            CommandFailed, CommandMissing, DaemonStopped, DaemonUnreachable, NoAddress,
        };
        // Unknown: a Funnel stays refused, anything else refuses until a look
        // succeeds.
        for reason in [CommandFailed, NoAddress, DaemonUnreachable] {
            for before in [Checking, Open, Unconfirmed] {
                assert_eq!(
                    lockout_after_failure(before, &reason),
                    Unconfirmed,
                    "{before:?} {reason:?}"
                );
            }
            assert_eq!(lockout_after_failure(Funnel, &reason), Funnel, "{reason:?}");
        }
        // An answer: no Tailscale here at all, or a daemon that is definitely
        // stopped (the probe only reports these when nothing else of Tailscale
        // is here). Nothing can publish dux, so it serves, except that a Funnel
        // already seen is only lifted by a look that SEES it gone.
        for reason in [CommandMissing, DaemonStopped] {
            for before in [Checking, Open, Unconfirmed] {
                assert_eq!(
                    lockout_after_failure(before, &reason),
                    Open,
                    "{before:?} {reason:?}"
                );
            }
            assert_eq!(
                lockout_after_failure(Funnel, &reason),
                Funnel,
                "a Funnel survives a daemon outage: {reason:?}"
            );
        }
    }

    /// A CLI that is missing while Tailscale is evidently here is its own
    /// refusal, because its way out is not fixing tailscaled: it is putting the
    /// command where dux looks.
    #[test]
    fn a_missing_cli_with_tailscale_here_refuses_with_its_own_way_out() {
        use crate::host_guard::FunnelLockout::{Checking, CliNotFound, Funnel, Open, Unconfirmed};
        for before in [Checking, Open, Unconfirmed, CliNotFound] {
            assert_eq!(
                lockout_after_failure(before, &TailscaleUnavailable::Unverifiable),
                CliNotFound,
                "{before:?}"
            );
        }
        assert_eq!(
            lockout_after_failure(Funnel, &TailscaleUnavailable::Unverifiable),
            Funnel
        );
        let (tone, text) =
            lockout_news(Open, CliNotFound, Because::NoTailscaleHere).expect("refusing is news");
        assert_eq!(tone, dux_core::statusline::StatusTone::Warning);
        let refusal = CliNotFound.refusal().expect("it refuses");
        for body in [text.as_str(), refusal] {
            for needle in [
                "PATH",
                "/usr/local/bin",
                "/Applications/Tailscale.app",
                "tailscale = \"no\"",
                "turns off",
                "no login",
            ] {
                assert!(body.contains(needle), "{needle}: {body}");
            }
            assert!(!body.contains("tailscale status"), "{body}");
        }
        assert!(lockout_news(CliNotFound, Open, Because::Look).is_some());
        assert!(lockout_news(Unconfirmed, CliNotFound, Because::Look).is_some());
    }

    /// A node that is down cannot run `tailscale funnel ... off` (it needs the
    /// tailnet), so a saved Funnel to dux refuses with its own way out:
    /// `tailscale up` first, then the Funnel off, then `no` as a last resort.
    #[test]
    fn a_saved_funnel_on_a_node_that_is_down_says_to_bring_it_up_first() {
        use crate::host_guard::FunnelLockout::{Checking, Funnel, FunnelSaved, Open, Unconfirmed};
        let down = TailscaleIdentity {
            node_down: true,
            ..tcp_funnelled("box.tail.ts.net")
        };
        assert_eq!(lockout_after_look(&down), FunnelSaved);
        assert_eq!(
            lockout_after_look(&tcp_funnelled("box.tail.ts.net")),
            Funnel
        );
        let down_clear = TailscaleIdentity {
            node_down: true,
            ..identity("box.tail.ts.net", &[])
        };
        assert_eq!(lockout_after_look(&down_clear), Open);
        // It survives anything short of a look that shows the Funnel gone.
        for reason in [
            TailscaleUnavailable::CommandMissing,
            TailscaleUnavailable::DaemonStopped,
            TailscaleUnavailable::CommandFailed,
            TailscaleUnavailable::Unverifiable,
        ] {
            assert_eq!(lockout_after_failure(FunnelSaved, &reason), FunnelSaved);
        }
        let (tone, news) = lockout_news(Open, FunnelSaved, Because::Look).expect("news");
        assert_eq!(tone, dux_core::statusline::StatusTone::Warning);
        let refusal = FunnelSaved.refusal().expect("it refuses");
        for body in [news.as_str(), refusal] {
            let up = body.find("tailscale up").expect("names tailscale up");
            let off = body
                .find("tailscale funnel")
                .expect("names turning the Funnel off");
            let no = body
                .find("tailscale = \"no\"")
                .expect("names the last resort");
            assert!(up < off && off < no, "{body}");
            assert!(
                body.contains("no login") && body.contains("turns off"),
                "{body}"
            );
        }
        for before in [Checking, Open, Unconfirmed, Funnel] {
            assert!(lockout_news(before, FunnelSaved, Because::Look).is_some());
        }
        assert!(lockout_news(FunnelSaved, Open, Because::Look).is_some());
        assert!(lockout_news(FunnelSaved, Checking, Because::Look).is_none());
    }

    #[test]
    fn a_funnel_to_dux_is_the_lockouts_news_not_the_names() {
        // One sentence for one event: the lockout says it, the name news does not
        // say it again.
        let plain = identity("box.tail.ts.net", &[]);
        assert!(identity_news(Some(&plain), &tcp_funnelled("box.tail.ts.net")).is_empty());
        assert!(identity_news(Some(&tcp_funnelled("box.tail.ts.net")), &plain).is_empty());
    }

    #[test]
    fn switching_to_no_says_out_loud_that_it_lifted_a_refusal() {
        let text = lockout_lifted_by_no();
        assert!(text.contains("no longer checks"), "{text}");
        assert!(text.contains("Funnel"), "{text}");
        assert!(text.contains("no login"), "{text}");
    }

    #[test]
    fn the_news_follows_the_name_dux_admits_not_the_one_tailscale_reports() {
        // A Headscale rename: dux never answered to either name.
        assert!(identity_news(Some(&headscale("box")), &headscale("box2")).is_empty());
        // A Headscale name going away: dux never answered to it.
        let mut gone = headscale("box");
        gone.status.dns_name = None;
        assert!(identity_news(Some(&headscale("box")), &gone).is_empty());
        // A rename under Funnel: dux answers to neither, and only Funnel is news.
        let before = identity("box.old-tailnet.ts.net", &[]);
        let after = funnelled_elsewhere("box.example-tailnet.ts.net");
        let news = identity_news(Some(&before), &after);
        assert_eq!(news.len(), 1, "{news:?}");
        assert!(news[0].1.contains("Funnel"), "{news:?}");
        assert!(!news[0].1.contains("now answers"), "{news:?}");
    }

    #[test]
    fn a_name_tailscale_did_not_assign_is_never_admitted_by_itself() {
        let mut headscale = identity("box.vpn.example.com", &[]);
        headscale.status.magic_dns_suffix = Some("vpn.example.com".to_string());
        assert_eq!(admitted_own_name(&headscale), None);
        let mut no_suffix = identity("box.tail.ts.net", &[]);
        no_suffix.status.magic_dns_suffix = None;
        assert_eq!(admitted_own_name(&no_suffix), None);
    }

    #[test]
    fn a_failed_look_after_a_good_one_withdraws_the_name() {
        // Fail closed: the guard must not keep admitting a name on an answer
        // that may be out of date, because the look that failed is the one that
        // would have seen a Funnel switched on.
        let a = identity("box.tail.ts.net", &[]);
        assert_eq!(
            run_identity_script(vec![
                Ok(a.clone()),
                Err(TailscaleUnavailable::CommandFailed),
                Ok(a.clone()),
            ]),
            vec![
                WatchEvent::Identity(a.clone()),
                WatchEvent::IdentityFailed(TailscaleUnavailable::CommandFailed),
                WatchEvent::Identity(a),
            ]
        );
    }

    #[test]
    fn every_failed_look_is_reported_with_its_reason() {
        // The serve loop tells "nothing could publish dux" (no CLI, no daemon)
        // from "dux could not find out", so every failure goes back with why.
        assert_eq!(
            run_identity_script(vec![
                Err(TailscaleUnavailable::CommandFailed),
                Err(TailscaleUnavailable::DaemonUnreachable),
            ]),
            vec![
                WatchEvent::IdentityFailed(TailscaleUnavailable::CommandFailed),
                WatchEvent::IdentityFailed(TailscaleUnavailable::DaemonUnreachable),
            ]
        );
    }

    #[test]
    fn a_watcher_for_the_name_alone_never_looks_for_the_address() {
        // On `yes` the address is looked up once and never again, but the name
        // is watched for as long as the guard can admit it, so a Funnel switched
        // on later is noticed.
        let detected = std::sync::atomic::AtomicUsize::new(0);
        let looks = std::sync::atomic::AtomicUsize::new(0);
        let sent = Mutex::new(Vec::new());
        watch_tailscale(&Watch {
            primary: addr("127.0.0.1:8080"),
            period: Duration::ZERO,
            probe_first: false,
            addresses: false,
            detect: &|| {
                detected.fetch_add(1, Ordering::SeqCst);
                Ok(ip("100.64.0.5"))
            },
            identify: &|| {
                let n = looks.fetch_add(1, Ordering::SeqCst);
                Ok(if n == 0 {
                    identity("box.tail.ts.net", &[])
                } else {
                    funnelled_elsewhere("box.tail.ts.net")
                })
            },
            bound: &|| None,
            known_identity: &|| match sent.lock().unwrap().last() {
                Some(WatchEvent::Identity(id)) => Some(id.clone()),
                _ => None,
            },
            emit: &|event| {
                sent.lock().unwrap().push(event);
                true
            },
            stop: &|| looks.load(Ordering::SeqCst) >= 3,
        });
        assert_eq!(detected.load(Ordering::SeqCst), 0, "no address look at all");
        assert_eq!(
            sent.into_inner().unwrap(),
            vec![
                WatchEvent::Identity(identity("box.tail.ts.net", &[])),
                WatchEvent::Identity(funnelled_elsewhere("box.tail.ts.net")),
            ]
        );
    }

    #[test]
    fn a_first_look_that_finds_funnel_warns_at_once() {
        let funnelled = identity("box.tail.ts.net", &[("https://box.tail.ts.net", true)]);
        let news = identity_news(None, &funnelled);
        assert_eq!(news.len(), 1, "{news:?}");
        assert_eq!(news[0].0, dux_core::statusline::StatusTone::Warning);
    }

    // ── A live mode change ────────────────────────────────────────────────

    #[test]
    fn every_mode_transition_plans_the_steps_that_mode_needs() {
        let ts = addr("100.64.0.5:8080");
        use TailscaleMode::{Auto, No, Yes};

        // → no: stop watching, drop the leg, and stop admitting Tailscale Host
        // literals, in that order.
        assert_eq!(
            plan_mode_change(Auto, No, Some(ts), false),
            vec![
                ModeStep::StopWatcher,
                ModeStep::Unbind(ts),
                ModeStep::SetHostLiterals(false),
                ModeStep::ForgetIdentity,
            ]
        );
        assert_eq!(
            plan_mode_change(Yes, No, Some(ts), false),
            vec![
                ModeStep::StopWatcher,
                ModeStep::Unbind(ts),
                ModeStep::SetHostLiterals(false),
                ModeStep::ForgetIdentity,
            ],
            "yes watches the name, so that watcher stops too"
        );
        assert_eq!(
            plan_mode_change(No, No, None, false),
            vec![ModeStep::SetHostLiterals(false), ModeStep::ForgetIdentity]
        );

        // → yes: a one-shot detection, with the literals opened first so a
        // tailnet browser is not refused between the two steps.
        assert_eq!(
            plan_mode_change(No, Yes, None, false),
            vec![ModeStep::SetHostLiterals(true), ModeStep::DetectAndBind]
        );
        assert_eq!(
            plan_mode_change(Auto, Yes, Some(ts), false),
            vec![
                ModeStep::StopWatcher,
                ModeStep::SetHostLiterals(true),
                ModeStep::DetectAndBind,
            ]
        );

        // → auto: a watcher whose first probe is immediate, so the command has a
        // visible outcome rather than one a period later.
        assert_eq!(
            plan_mode_change(No, Auto, None, false),
            vec![
                ModeStep::SetHostLiterals(true),
                ModeStep::StartWatcher { probe_now: true },
            ]
        );
        assert_eq!(
            plan_mode_change(Yes, Auto, Some(ts), false),
            vec![
                ModeStep::SetHostLiterals(true),
                ModeStep::StartWatcher { probe_now: true },
            ],
            "the bound leg is left alone; the watcher reconciles it"
        );
        // auto → auto replaces the watcher rather than adding a second one, and
        // the start is what replaces it: an explicit stop as well would be a
        // second one.
        assert_eq!(
            plan_mode_change(Auto, Auto, Some(ts), false),
            vec![
                ModeStep::SetHostLiterals(true),
                ModeStep::StartWatcher { probe_now: true },
            ]
        );
        // yes → yes still re-detects: the user asked for the address to be
        // looked up again, which is the only thing "yes" does.
        assert_eq!(
            plan_mode_change(Yes, Yes, None, false),
            vec![
                ModeStep::StopWatcher,
                ModeStep::SetHostLiterals(true),
                ModeStep::DetectAndBind,
            ]
        );
    }

    #[test]
    fn a_run_started_with_no_tailscale_refuses_every_mode_that_wants_it() {
        use TailscaleMode::{Auto, No, Yes};
        for next in [Auto, Yes] {
            assert_eq!(
                plan_mode_change(No, next, None, true),
                vec![ModeStep::Refuse],
                "--no-tailscale outranks a live {next:?}"
            );
        }
        // Asking for the mode the run is already in is not a refusal: the
        // ordinary plan runs, and on a forced-no run it has nothing to undo.
        assert_eq!(
            plan_mode_change(No, No, None, true),
            vec![ModeStep::SetHostLiterals(false), ModeStep::ForgetIdentity]
        );
    }

    #[test]
    fn a_probe_first_watcher_checks_before_it_parks() {
        // The palette command has to have a visible outcome, so a watcher started
        // by a live switch to `auto` cannot wait out a whole period before its
        // first look. The period here is an hour: a watcher that parked first
        // would still be parked, so the bounded receive below is what fails
        // rather than any assertion about elapsed time.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let calls = Mutex::new(0usize);
            watch_legs_only(
                addr("127.0.0.1:8080"),
                Duration::from_secs(3600),
                true,
                &|| {
                    *calls.lock().unwrap() += 1;
                    Ok(ip("100.64.0.5"))
                },
                &|| None,
                &|_| true,
                // Stops once a probe has happened, so the loop ends by itself the
                // moment the immediate probe is done.
                &|| *calls.lock().unwrap() >= 1,
            );
            let _ = tx.send(*calls.lock().unwrap());
        });
        let probes = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a probe_first watcher must probe before it parks");
        assert_eq!(probes, 1, "exactly one probe, then the stop flag ends it");
    }

    #[test]
    fn a_watcher_stopped_mid_probe_never_emits_what_it_found() {
        // A watcher parked in a five-second detection while the mode flips to
        // `no` would otherwise come back and re-bind the leg that was just
        // dropped. The stop flag is checked again after the probe.
        let stopped = AtomicBool::new(false);
        let sent = Mutex::new(Vec::new());
        watch_legs_only(
            addr("127.0.0.1:8080"),
            Duration::ZERO,
            true,
            &|| {
                stopped.store(true, Ordering::SeqCst);
                Ok(ip("100.64.0.5"))
            },
            &|| None,
            &|cmd| {
                sent.lock().unwrap().push(cmd);
                true
            },
            &|| stopped.load(Ordering::SeqCst),
        );
        assert!(
            sent.lock().unwrap().is_empty(),
            "a stopped watcher must not emit the command it had already planned"
        );
    }

    // ── The waiting note ──────────────────────────────────────────────────

    #[test]
    fn the_waiting_note_appears_only_on_auto_with_nothing_detected() {
        let note =
            waiting_note(TailscaleMode::Auto, StartupLeg::Undetected).expect("auto, no address");
        assert!(note.contains("waiting for the interface"), "{note}");
        assert!(note.contains("auto"), "must name the mode: {note}");
        assert_eq!(
            waiting_note(TailscaleMode::Auto, StartupLeg::Bound),
            None,
            "it bound"
        );
        assert_eq!(
            waiting_note(TailscaleMode::Yes, StartupLeg::Undetected),
            None,
            "yes gets the settled-for-this-run warning instead, not a waiting note"
        );
        assert_eq!(
            waiting_note(TailscaleMode::No, StartupLeg::Undetected),
            None,
            "not wanted"
        );
    }

    #[test]
    fn a_detected_address_that_would_not_bind_is_not_reported_as_waiting_for_it() {
        // The interface is right there; saying dux is waiting for it would send the
        // operator looking at Tailscale instead of at whatever holds the port.
        let note =
            waiting_note(TailscaleMode::Auto, StartupLeg::BindFailed).expect("auto, failed bind");
        assert!(
            !note.contains("waiting for the interface"),
            "the interface is present, so this must not claim otherwise: {note}"
        );
        assert!(
            note.contains("would not bind"),
            "must name what actually happened: {note}"
        );
        assert!(
            note.contains("tries again"),
            "must say dux retries by itself: {note}"
        );
        // The static modes still say nothing here: the bind warning row already
        // carries the failure, and nothing is going to retry it.
        assert_eq!(
            waiting_note(TailscaleMode::Yes, StartupLeg::BindFailed),
            None
        );
        assert_eq!(
            waiting_note(TailscaleMode::No, StartupLeg::BindFailed),
            None
        );
    }

    // ── A best-effort leg's death ─────────────────────────────────────────

    #[test]
    fn a_watched_leg_promises_a_re_bind_and_an_unwatched_one_does_not() {
        // The message is the only thing the operator sees, so it must not promise a
        // recovery that cannot happen: on `yes` nothing is watching the interface,
        // so the leg is down until a restart or a mode change.
        let ts = addr("100.64.0.5:8080");
        let err = anyhow::anyhow!("connection reset by peer");

        let watched = best_effort_death_warning(ts, &err, true);
        assert!(watched.contains("100.64.0.5:8080"), "{watched}");
        assert!(watched.contains("connection reset"), "{watched}");
        assert!(
            watched.contains("by itself"),
            "an auto run really does re-bind it: {watched}"
        );

        let unwatched = best_effort_death_warning(ts, &err, false);
        assert!(
            !unwatched.contains("by itself"),
            "nothing is watching, so nothing binds it back: {unwatched}"
        );
        assert!(
            unwatched.contains("restart dux") && unwatched.contains("\"auto\""),
            "must name both ways out: {unwatched}"
        );
    }

    // ── The parent lane ───────────────────────────────────────────────────

    #[test]
    fn record_serve_failure_first_caller_wins_and_triggers_shutdown() {
        // The first serve task to die records its error, arms the flag, and trips
        // the shutdown watch; a later caller (another listener winding down) does
        // NOT overwrite the first error but STILL nudges shutdown. This is the F5
        // load-bearing logic, tested directly because forcing a real axum accept
        // loop to error mid-serve is inherently flaky. Exercised through the ONE
        // shared [`ServeShutdown`] primitive every serve path uses.
        let shutdown = ServeShutdown::for_watched(true);
        let mut shutdown_rx = shutdown.subscribe();

        let first = shutdown.record_failure(anyhow::anyhow!("listener A died"));
        assert!(first, "the first failure must win");
        assert!(shutdown.is_failed(), "the flag must be armed");
        assert!(
            *shutdown_rx.borrow_and_update(),
            "the shutdown watch must be tripped so other listeners wind down"
        );

        // A second listener failing afterwards must NOT clobber the first error,
        // but still no-ops the shutdown send (idempotent).
        let second = shutdown.record_failure(anyhow::anyhow!("listener B died"));
        assert!(!second, "a later failure is not the first-error winner");
        assert_eq!(
            shutdown.take_error().unwrap().to_string(),
            "listener A died",
            "the first error is preserved"
        );
        // After taking it, the slot is empty.
        assert!(
            shutdown.take_error().is_none(),
            "the error slot is drained by take_error"
        );
    }

    #[tokio::test]
    async fn serve_shutdown_trigger_resolves_waiters() {
        // The watch lane is the graceful-shutdown trigger every serve task awaits:
        // a plain `trigger()` (a SIGINT/SIGTERM or the flip's engine loop exiting)
        // must resolve `wait_for_shutdown` WITHOUT recording any error, so a clean
        // stop is not mistaken for a listener death.
        let shutdown = ServeShutdown::for_watched(true);
        let waiter = shutdown.subscribe();
        shutdown.trigger();
        // Resolves promptly (bounded so a regression fails rather than hangs).
        tokio::time::timeout(Duration::from_secs(1), wait_for_shutdown(waiter))
            .await
            .expect("a triggered shutdown must resolve waiters");
        assert!(!shutdown.is_failed(), "a clean trigger is not a failure");
        assert!(
            shutdown.take_error().is_none(),
            "a clean trigger records no error"
        );
    }

    #[tokio::test]
    async fn serve_shutdown_failure_winds_down_a_sibling_listener() {
        // A genuine first-error wind-down end to end: a real bound listener serves
        // a trivial app whose graceful-shutdown future awaits the shared watch.
        // When a SIBLING records a failure, the watch trips and this listener's
        // serve future resolves (Ok, graceful), proving one listener's death winds
        // the others down. This is the run_plain_http first-error behavior
        // exercised over a real accept loop (cheap, deterministic: no flaky
        // mid-serve error injection needed, we trip the lane the sibling would).
        let shutdown = ServeShutdown::for_watched(true);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let app = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let task_shutdown = shutdown.subscribe();
        let serve = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(wait_for_shutdown(task_shutdown))
                .await
        });

        // A sibling listener died: record it. The watch trips, so the serving task
        // above winds down gracefully.
        let first = shutdown.record_failure(anyhow::anyhow!("sibling listener failed"));
        assert!(first, "the first failure wins");

        let joined = tokio::time::timeout(Duration::from_secs(2), serve)
            .await
            .expect("the sibling listener must wind down once the watch trips")
            .expect("serve task joins");
        assert!(
            joined.is_ok(),
            "a graceful shutdown returns Ok even though a sibling failed"
        );
        // The recorded error is still available for the caller to surface.
        assert_eq!(
            shutdown.take_error().unwrap().to_string(),
            "sibling listener failed"
        );
    }

    // ── Leg lanes ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn stopping_one_leg_leaves_the_parent_and_its_siblings_alone() {
        let shutdown = ServeShutdown::for_watched(true);
        let ts = addr("100.64.0.5:8080");
        let leg = shutdown.register_leg(ts);
        let parent = shutdown.subscribe();

        assert!(shutdown.stop_leg(ts), "a live leg is stopped");
        tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_leg_shutdown(parent.clone(), leg),
        )
        .await
        .expect("the stopped leg's waiter must resolve");

        assert!(!*parent.clone().borrow_and_update(), "parent untouched");
        assert!(!shutdown.is_failed(), "a leg stop is not a failure");
        assert!(shutdown.take_error().is_none());
        assert!(
            !shutdown.stop_leg(ts),
            "stopping a leg twice reports nothing to stop"
        );
    }

    #[tokio::test]
    async fn a_parent_trip_fans_out_to_a_leg_that_only_waits_on_its_own_lane() {
        // The flip-teardown-while-a-leg-is-parked case. The leg was added AFTER
        // serving started, so it never saw the parent's initial state; if the
        // trigger did not fan out, its listener would still be holding the socket
        // when the TUI came back.
        let shutdown = ServeShutdown::for_watched(true);
        let ts = addr("100.64.0.5:8080");
        let leg = shutdown.register_leg(ts);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let app = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let parent = shutdown.subscribe();
        let mut leg_state = leg.clone();
        let serve = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(wait_for_leg_shutdown(parent, leg))
                .await
        });

        shutdown.trigger();
        let joined = tokio::time::timeout(Duration::from_secs(2), serve)
            .await
            .expect("a teardown must wind the parked leg down")
            .expect("serve task joins");
        // The serve future RESOLVED, which is axum's contract for "stopped
        // accepting and dropped the listener", and it resolved through the leg's
        // OWN lane, which is the fan-out this test is about.
        assert!(joined.is_ok(), "a graceful shutdown returns Ok");
        assert!(
            *leg_state.borrow_and_update(),
            "the parent trigger must have tripped the leg's own lane"
        );
    }

    #[tokio::test]
    async fn a_best_effort_leg_death_is_isolated_but_a_required_one_is_fatal() {
        let shutdown = ServeShutdown::for_watched(true);
        let ts = addr("100.64.0.5:8080");
        let _leg = shutdown.register_leg(ts);
        let parent = shutdown.subscribe();
        assert!(shutdown.has_leg(ts), "a registered leg is live");

        shutdown.record_best_effort_failure(ts, &anyhow::anyhow!("tailscale listener died"));
        assert!(
            !shutdown.has_leg(ts),
            "the registry is what the serve loop reconciles against, so a leg that \
             died must no longer look live"
        );
        assert!(
            !*parent.clone().borrow_and_update(),
            "a best-effort death must not trip the parent"
        );
        assert!(!shutdown.is_failed(), "and must not arm the failure flag");
        assert!(
            shutdown.take_error().is_none(),
            "and must not become the serve's reported error"
        );
        assert!(
            !shutdown.stop_leg(ts),
            "the dead leg is no longer registered"
        );

        // A required leg's death still ends everything, with its error kept.
        assert!(shutdown.record_failure(anyhow::anyhow!("loopback listener died")));
        assert!(shutdown.is_failed());
        assert!(*parent.clone().borrow_and_update());
        assert_eq!(
            shutdown.take_error().unwrap().to_string(),
            "loopback listener died"
        );
    }

    #[tokio::test]
    async fn re_registering_an_address_trips_the_lane_it_replaces() {
        // Defence in depth: if a leg were ever registered twice for one address,
        // the listener behind the first lane must not be left unstoppable.
        let shutdown = ServeShutdown::for_watched(true);
        let ts = addr("100.64.0.5:8080");
        let first = shutdown.register_leg(ts);
        let _second = shutdown.register_leg(ts);
        tokio::time::timeout(Duration::from_secs(1), wait_for_shutdown(first))
            .await
            .expect("the replaced lane must be tripped");
    }
}
