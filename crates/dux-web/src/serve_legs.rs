//! Per-leg listener lifecycle: the shutdown primitive every serve path shares,
//! the registry of live legs and their individual stop lanes, and the Tailscale
//! interface watcher that adds and drops the Tailscale leg while dux keeps
//! serving.
//!
//! ## Why a leg is a thing
//!
//! dux serves one router on several listeners: the REQUIRED one the operator
//! named, and the BEST-EFFORT Tailscale one. Those two do not deserve the same
//! treatment. The required listener dying means the server is over. The Tailscale
//! listener dying is Tuesday: the laptop suspended, the daemon restarted, the
//! user logged out of their tailnet. So each listener gets its own stop lane and
//! its own failure verdict, and one leg can end without taking the server with
//! it.
//!
//! ## The two directions
//!
//! - The PARENT trip (a signal, a required leg's death, the flip's engine loop
//!   returning) fans out over every leg lane, so nothing is left holding a socket
//!   after a teardown.
//! - A LEG trip (the interface went away) stops exactly one listener and leaves
//!   the parent alone.
//!
//! Every serve future therefore waits on both its own lane and the parent's.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dux_core::config::TailscaleMode;
use dux_core::tailscale::TailscaleUnavailable;

/// How often the watcher asks whether the Tailscale address is there.
///
/// A constant, not a setting: this is an implementation cadence, not a
/// preference. Ten seconds is well inside the time it takes a person to notice a
/// laptop has come back and reach for a browser, and the probe it pays for is one
/// bounded local call to a local daemon.
///
/// This period is also the flap debounce. There is deliberately no second
/// hysteresis window on top of it: an interface that appears and disappears
/// faster than this produces at most one transition per period, and one that
/// flaps slower than this is not flapping, it is changing.
pub(crate) const WATCH_PERIOD: Duration = Duration::from_secs(10);

/// How long the watcher parks between checks of the stop flag. Small enough that
/// serving can end promptly, large enough that waiting costs a wakeup a second
/// and nothing else.
const WATCH_SLICE: Duration = Duration::from_millis(250);

/// The ONE serve-shutdown primitive shared by all serve paths. It bundles the
/// first-error bookkeeping with the `watch<bool>` parent lane every listener
/// awaits AND the registry of per-leg lanes, so a single dying listener winds the
/// siblings down identically everywhere while a best-effort leg can be stopped on
/// its own:
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
    tailscale_watched: bool,
}

impl ServeShutdown {
    /// `tailscale_watched` is [`TailscaleMode::watches_interface`] for this run.
    /// The registry deliberately knows nothing about config modes, so the serve
    /// path states the fact once here instead of every leg restating it.
    pub(crate) fn new(tailscale_watched: bool) -> Self {
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        Self {
            failed: Arc::new(AtomicBool::new(false)),
            error: Arc::new(std::sync::Mutex::new(None)),
            shutdown_tx,
            legs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            tailscale_watched,
        }
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
    pub(crate) fn record_best_effort_failure(&self, addr: SocketAddr, err: &anyhow::Error) {
        dux_core::logger::warn(&format!(
            "[server] {}",
            best_effort_death_warning(addr, err, self.tailscale_watched)
        ));
        self.forget_leg(addr);
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

/// What the watcher asks the serve loop to do. One command per detect period at
/// most, except a genuine address CHANGE, which is an unbind and a bind of the
/// same leg and is sent as both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegCommand {
    /// The Tailscale interface is there at this address: bind and serve it,
    /// best-effort.
    Bind(SocketAddr),
    /// This Tailscale address is gone: stop that listener. Live sockets on it die
    /// with the listener, and the browser's ordinary reconnect is the recovery.
    Unbind(SocketAddr),
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
    if prev.watches_interface() {
        steps.push(ModeStep::StopWatcher);
    }
    match next {
        TailscaleMode::No => {
            if let Some(addr) = bound {
                steps.push(ModeStep::Unbind(addr));
            }
            steps.push(ModeStep::SetHostLiterals(false));
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

/// Run the watch loop: poll the detector, compare against what is bound, emit at
/// most one transition per period, and stop when `stop` says serving is over.
///
/// Every collaborator is injected so the whole loop is testable with no Tailscale
/// binary, no sockets and no clock: `detect` is the probe, `bound` reports what
/// the serve loop currently has bound (so a FAILED bind is retried next period
/// rather than being lost), `emit` hands a command to the serve loop and returns
/// false when nobody is listening any more, and `stop` ends the loop.
///
/// `probe_first` decides whether the loop starts by probing or by SLEEPING. A
/// watcher started at serve time sleeps: the startup bind answered the same
/// question a moment ago. A watcher started by a live switch to `auto` probes,
/// because the gesture that started it needs an outcome to report.
///
/// `stop` is consulted again after the probe and before the emit: a detection
/// can take seconds, and a watcher stopped during one must not hand the serve
/// loop a command for the mode it just left.
pub(crate) fn watch_tailscale_leg(
    primary: SocketAddr,
    period: Duration,
    probe_first: bool,
    detect: &dyn Fn() -> Result<IpAddr, TailscaleUnavailable>,
    bound: &dyn Fn() -> Option<SocketAddr>,
    emit: &dyn Fn(LegCommand) -> bool,
    stop: &dyn Fn() -> bool,
) {
    let mut immediate = probe_first;
    loop {
        if immediate {
            immediate = false;
            if stop() {
                return;
            }
        } else if !park(period, stop) {
            return;
        }
        let desired = desired_leg(primary, detect());
        let step = plan_leg_step(bound(), desired);
        if stop() {
            return;
        }
        let sent = match step {
            LegStep::Nothing => true,
            LegStep::Bind(addr) => emit(LegCommand::Bind(addr)),
            LegStep::Unbind(addr) => emit(LegCommand::Unbind(addr)),
            LegStep::Rebind { old, new } => {
                emit(LegCommand::Unbind(old)) && emit(LegCommand::Bind(new))
            }
        };
        if !sent {
            // The serve loop is gone; there is nobody left to tell.
            return;
        }
    }
}

/// Sleep for `period` in slices, returning false as soon as `stop` says to end.
/// Slicing is what makes a ten-second period compatible with a prompt teardown.
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
            watch_tailscale_leg(
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
        watch_tailscale_leg(
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

    #[test]
    fn the_stop_flag_ends_the_watcher_before_it_probes() {
        let calls = Mutex::new(0usize);
        watch_tailscale_leg(
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
            ]
        );
        assert_eq!(
            plan_mode_change(Yes, No, Some(ts), false),
            vec![ModeStep::Unbind(ts), ModeStep::SetHostLiterals(false)],
            "nothing was watching, so there is no watcher to stop"
        );
        assert_eq!(
            plan_mode_change(No, No, None, false),
            vec![ModeStep::SetHostLiterals(false)]
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
        // visible outcome rather than one ten seconds later.
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
        // auto → auto replaces the watcher rather than adding a second one.
        assert_eq!(
            plan_mode_change(Auto, Auto, Some(ts), false),
            vec![
                ModeStep::StopWatcher,
                ModeStep::SetHostLiterals(true),
                ModeStep::StartWatcher { probe_now: true },
            ]
        );
        // yes → yes still re-detects: the user asked for the address to be
        // looked up again, which is the only thing "yes" does.
        assert_eq!(
            plan_mode_change(Yes, Yes, None, false),
            vec![ModeStep::SetHostLiterals(true), ModeStep::DetectAndBind]
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
            vec![ModeStep::SetHostLiterals(false)]
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
            watch_tailscale_leg(
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
        watch_tailscale_leg(
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
        let shutdown = ServeShutdown::new(true);
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
        let shutdown = ServeShutdown::new(true);
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
        let shutdown = ServeShutdown::new(true);
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
        let shutdown = ServeShutdown::new(true);
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
        let shutdown = ServeShutdown::new(true);
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
        let shutdown = ServeShutdown::new(true);
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
        let shutdown = ServeShutdown::new(true);
        let ts = addr("100.64.0.5:8080");
        let first = shutdown.register_leg(ts);
        let _second = shutdown.register_leg(ts);
        tokio::time::timeout(Duration::from_secs(1), wait_for_shutdown(first))
            .await
            .expect("the replaced lane must be tripped");
    }
}
