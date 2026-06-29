//! The web layer: exposes the `dux-core` engine over HTTP/WebSocket so a browser
//! SPA can drive the same agent sessions the TUI does.
//!
//! ## Entry points
//!
//! - [`run_server`] — the `dux server` CLI path. Boots the engine on its own
//!   thread and serves axum on a self-built tokio runtime until SIGINT/SIGTERM.
//! - [`serve_with_engine`] — the in-process TUI↔server flip. Serves the web UI
//!   over an EXISTING live engine (PTYs intact) on the caller's thread, returning
//!   the engine when serving stops so the TUI can resume around the same agents.
//!
//! ## Major pieces
//!
//! - [`server`] — the axum router (all routes plain; dux is trusted-local with no
//!   login gate) and the same-origin WebSocket check, plus the `/ws` bridge to the
//!   engine.
//! - [`engine_actor`] — the `EngineHandle` and the request/drain loop that owns
//!   the `!Send` engine on its thread.
//!
//! ## Dependency isolation
//!
//! This crate depends on `dux-core`, never `dux-tui`. Isolation is enforced by
//! the `dep-isolation` CI job, which runs `cargo tree -p dux-web` and fails if
//! any TUI-only crate appears.

pub mod bootstrap;
pub mod bootstrap_routes;
pub mod browse_routes;
pub mod changes;
pub mod changes_routes;
pub mod config_routes;
pub mod console;
pub mod engine_actor;
pub mod event_bus;
pub mod file_routes;
pub mod git_routes;
pub mod host_guard;
pub mod project_actions;
pub mod project_reads;
pub mod rest_common;
pub mod server;
pub mod session_actions;
pub mod spine_routes;
pub mod startup_logs;
pub mod terminal_actions;
pub mod web_assets;

/// Crate-wide test helpers shared by the per-module route test suites (a single
/// headless engine handle + a gated router builder), so each REST route module
/// can exercise its handlers without duplicating the bootstrap recipe.
#[cfg(test)]
pub(crate) mod test_support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use axum::serve::ListenerExt;
use dux_core::config::{DuxPaths, PlanAddr, ServerPlan};
use dux_core::engine::Engine;

use crate::console::{Banner, Console, ListenerRow};
use crate::engine_actor::LoopControl;
use crate::server::RouterParams;

/// Boot the engine on its own thread and serve the web UI on every address in
/// the plan (one axum task per listener, sharing the router/state). Blocking
/// entry — builds its own tokio runtime.
///
/// `version` is the dux crate version the binary passes in (`CARGO_PKG_VERSION`)
/// for the console banner header.
///
/// This is the ONLY surface that owns the [`Console`]: it is built here from the
/// engine's loaded `[server] color`/`access_log` and threaded into the serve
/// paths. The TUI flip ([`serve_with_engine`]) NEVER constructs a real console —
/// it keeps its themed status screen and must not print to stdout.
pub fn run_server(paths: DuxPaths, plan: ServerPlan, version: String) -> Result<()> {
    run_plain_http(paths, plan.addrs, version)
}

/// Build the `dux server` console from the engine's loaded config: detect color
/// from `[server] color` (warning on an unrecognized value, then honoring it as
/// `auto`), construct a real stdout console, and read the `access_log` toggle.
/// Returns `(console, access_log)`. Used by both CLI serve paths; the flip does
/// NOT call this (it uses [`Console::noop`]).
fn build_console(config: &dux_core::config::Config) -> (Console, bool) {
    let setting = &config.server.color;
    if !crate::console::is_known_color_setting(setting) {
        dux_core::logger::warn(&format!(
            "[server] color = \"{setting}\" is not one of auto/always/never — treating it as \
             \"auto\". Fix [server] color in config.toml to silence this."
        ));
        eprintln!(
            "WARNING: [server] color = \"{setting}\" is not auto/always/never — using \"auto\"."
        );
    }
    let color = crate::console::detect(setting);
    (Console::stdout(color), config.server.access_log)
}

/// The warning shown when a BEST-EFFORT (Tailscale) listener cannot bind because
/// something else already holds that address. Names the address, the cause, and
/// BOTH remedies (stop the other process, or change the port). Emitted as a
/// `dux.log` WARN line. Pure so it is unit-testable.
fn tailscale_bind_warning(addr: SocketAddr, err: &std::io::Error) -> String {
    format!(
        "could not bind the Tailscale address {addr}: {err} — something else is already \
         listening there; serving on the remaining address(es) only. Stop that process or \
         change [server].port to also serve on Tailscale."
    )
}

/// A successfully bound listener paired with its requested address (so the URL
/// list is computed from what ACTUALLY bound, not what was requested).
/// `required` is the [`PlanAddr`] tag, retained so the post-bind banner can label
/// a best-effort leg (the LOCAL MODE Tailscale address) as "Tailscale" and a
/// required non-loopback leg as a plain public address.
#[derive(Debug)]
struct BoundListener {
    addr: SocketAddr,
    required: bool,
    listener: tokio::net::TcpListener,
}

/// Bind every [`PlanAddr`], honoring its required/best-effort tag.
///
/// - REQUIRED (the configured `host:port` or an explicit `--bind`): a bind
///   failure is FATAL — it logs a `logger::error` with the failing address and
///   returns the error (with address context) so the serve aborts. This is the
///   explicit-failure tenet: the operator named this address.
/// - BEST-EFFORT (the Tailscale leg of LOCAL MODE): a bind failure logs a WARN
///   naming the address, the cause, and both remedies, collects the SAME text in
///   the returned warnings vec, and CONTINUES without that listener.
///
/// If NOTHING binds (every address failed) the whole serve is fatal — there is
/// nothing left to serve. Returns the bound listeners (with their addresses) and
/// the best-effort warnings (the caller logs them to `dux.log`; they are not
/// re-broadcast — see [`run_plain_http`] for why a startup broadcast reaches no
/// clients). The returned vec is retained because the bind tests assert on it.
async fn bind_plan_addrs(addrs: &[PlanAddr]) -> Result<(Vec<BoundListener>, Vec<String>)> {
    let mut bound = Vec::with_capacity(addrs.len());
    let mut warnings = Vec::new();
    for plan_addr in addrs {
        let addr = plan_addr.addr();
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => bound.push(BoundListener {
                addr,
                required: plan_addr.is_required(),
                listener,
            }),
            Err(err) if plan_addr.is_required() => {
                // The operator named this address; refuse to serve silently
                // without it. Log with address context, then propagate the error.
                dux_core::logger::error(&format!(
                    "[server] could not bind the listen address {addr}: {err} — something else \
                     is already listening there. Stop that process or change the configured \
                     address/port."
                ));
                return Err(anyhow::anyhow!(
                    "could not bind the listen address {addr}: {err} \
                     (is something already listening there?)"
                ));
            }
            Err(err) => {
                // Best-effort (Tailscale) leg: warn loudly, keep serving the rest.
                let warning = tailscale_bind_warning(addr, &err);
                dux_core::logger::warn(&format!("[server] {warning}"));
                warnings.push(warning);
            }
        }
    }
    if bound.is_empty() {
        // Every address failed (e.g. a single required loopback that was busy is
        // handled above; this guards the all-best-effort edge and future shapes).
        anyhow::bail!(
            "could not bind any of the requested server addresses; nothing left to serve. \
             Check that the configured ports are free."
        );
    }
    Ok((bound, warnings))
}

/// The plain-HTTP serve path: one axum task per listener (loopback, Tailscale,
/// LAN, or proxy-fronted), sharing the router/state. Shutdown rides the
/// [`ServeShutdown`] watch lane: a SIGINT/SIGTERM trips the watch, and the FIRST
/// listener to die records its error and trips the watch too, so the siblings get
/// a graceful shutdown and the error propagates (genuine first-error wind-down —
/// no longer a no-abort JoinSet wait).
///
/// A BEST-EFFORT (Tailscale) address whose bind fails (a third-party process
/// already holds it) does NOT abort the serve: it warns loudly to `dux.log` and
/// the server keeps serving the remaining (bound) addresses. The warning is NOT
/// re-broadcast to web clients — the status broadcast has no replay, and clients
/// only subscribe when their WS connects, which is always after this startup bind,
/// so a startup broadcast would reach zero receivers. `dux.log` and the CLI
/// startup banner (which flags a best-effort leg) are the delivery surfaces for
/// the `dux server` path; the TUI palette flip delivers through its own status
/// line, unchanged.
/// Build the plain-HTTP startup banner from the BOUND legs (each an
/// `(addr, required)` pair). Each leg is labeled by what it is:
/// - loopback → "Local (loopback)"
/// - a best-effort (LOCAL MODE Tailscale) leg → "Tailscale"
/// - a required non-loopback leg (an explicit `--bind` public/LAN entry) →
///   "Listen"
///
/// Best-effort bind degradations (a busy Tailscale address) become ⚠ rows. Pure
/// (over `(SocketAddr, bool)` pairs, not the live listeners) so it is
/// unit-testable without binding sockets.
fn plain_http_banner(
    version: &str,
    bound: &[(SocketAddr, bool)],
    bind_warnings: &[String],
    security_note: Option<String>,
) -> Banner {
    let listeners = bound
        .iter()
        .map(|(addr, required)| {
            let label = if addr.ip().is_loopback() {
                "Local (loopback)"
            } else if !required {
                "Tailscale"
            } else {
                "Listen"
            };
            ListenerRow {
                label: label.to_string(),
                url: format!("http://{addr}"),
            }
        })
        .collect();
    Banner {
        version: version.to_string(),
        mode: "plain HTTP".to_string(),
        warnings: bind_warnings.to_vec(),
        listeners,
        security_note,
    }
}

/// How far the server can be reached, classified from the BOUND legs (each an
/// `(addr, required)` pair where `required` is true for explicit `--bind`
/// public/LAN entries and false for best-effort Tailscale local-mode legs).
/// Worst-wins: any required non-loopback leg makes it `Public`; otherwise any
/// best-effort non-loopback leg makes it `Tailscale`; otherwise `LoopbackOnly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reachability {
    /// Every bound leg is genuine loopback — nothing off-host can reach it.
    LoopbackOnly,
    /// A best-effort (Tailscale local-mode) non-loopback leg is bound, and no
    /// public/LAN leg is.
    Tailscale,
    /// A required non-loopback leg (an explicit `--bind` public/LAN entry)
    /// is bound.
    Public,
}

fn reachability(bound: &[(SocketAddr, bool)]) -> Reachability {
    let mut result = Reachability::LoopbackOnly;
    for (addr, required) in bound {
        if addr.ip().is_loopback() {
            continue;
        }
        if *required {
            return Reachability::Public;
        }
        result = Reachability::Tailscale;
    }
    result
}

/// Safety note shown when the server is reachable on the tailnet (loopback
/// primary + a best-effort Tailscale leg). Exported so the TUI flip path in
/// `crates/dux/src/main.rs` can reference the same text without a separate
/// copy.
pub const SAFETY_NOTE_TAILNET: &str = "Reachable by other devices on your tailnet (no login). \
     Disable with tailscale_enabled = false under [server].";

/// Safety note shown when the server is bound on a required non-loopback
/// (public/LAN) address. Exported alongside [`SAFETY_NOTE_TAILNET`] so both
/// operator-facing strings live in one place.
pub const SAFETY_NOTE_PUBLIC: &str = "Reachable on your network with NO login. \
     Anyone who can reach this address controls your agents and worktrees. \
     Put it behind Tailscale or a trusted reverse proxy.";

/// Suffix appended to [`SAFETY_NOTE_PUBLIC`] when a Tailscale best-effort leg
/// is ALSO bound alongside the required public/LAN primary.
pub const SAFETY_NOTE_TAILSCALE_ALSO_BOUND: &str = " (The Tailscale address is bound too.)";

/// Operator-facing safety note based on the bound addresses' reachability.
/// Returns None when the server is loopback-only (nothing to warn about).
/// Uses highest-severity-wins: a required non-loopback primary yields the LAN
/// warning regardless of whether a Tailscale leg is also bound.
pub fn safety_note(addrs: &[PlanAddr]) -> Option<String> {
    let pairs: Vec<(SocketAddr, bool)> =
        addrs.iter().map(|a| (a.addr(), a.is_required())).collect();
    match reachability(&pairs) {
        Reachability::LoopbackOnly => None,
        Reachability::Tailscale => Some(SAFETY_NOTE_TAILNET.to_string()),
        Reachability::Public => {
            let has_tailscale = pairs
                .iter()
                .any(|(addr, required)| !addr.ip().is_loopback() && !required);
            let mut msg = SAFETY_NOTE_PUBLIC.to_string();
            if has_tailscale {
                msg.push_str(SAFETY_NOTE_TAILSCALE_ALSO_BOUND);
            }
            Some(msg)
        }
    }
}

fn run_plain_http(paths: DuxPaths, addrs: Vec<PlanAddr>, version: String) -> Result<()> {
    let engine = bootstrap::bootstrap_engine(&paths)?;
    // Build the vite-style CLI console (color from [server] color) + the access-log
    // toggle before the engine moves into the actor thread.
    let (console, access_log) = build_console(&engine.config);
    // Capture the connection caps and allowed hosts before the engine moves into
    // the actor thread. Both are read-only config values the router builder needs.
    let max_ws_caps = (
        engine.config.server.max_websocket_events_connections,
        engine.config.server.max_websocket_agent_connections,
        engine.config.server.max_websocket_terminal_connections,
    );
    let engine_allowed_hosts = engine.config.server.allowed_hosts.clone();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        // Bind every address first, honoring the required/best-effort tags. A
        // failed REQUIRED bind aborts here (with the address logged + in the
        // error); a failed BEST-EFFORT (Tailscale) bind is dropped with a warning
        // and the server proceeds on the rest. The best-effort warnings ride into
        // the post-bind banner as ⚠ rows (and are already in dux.log).
        let (bound, bind_warnings) = bind_plan_addrs(&addrs).await?;

        // Post-bind banner: built from what ACTUALLY bound, so it shows truth (no
        // pre-bind hedging). Replaces main.rs's pre-bind URL println. Project the
        // bound listeners into (addr, required) pairs for the pure banner builder.
        let banner_legs: Vec<(SocketAddr, bool)> =
            bound.iter().map(|b| (b.addr, b.required)).collect();
        let bound_plan_addrs: Vec<PlanAddr> = bound
            .iter()
            .map(|b| {
                if b.required {
                    PlanAddr::required(b.addr)
                } else {
                    PlanAddr::best_effort(b.addr)
                }
            })
            .collect();
        let note = safety_note(&bound_plan_addrs);
        console.banner(&plain_http_banner(
            &version,
            &banner_legs,
            &bind_warnings,
            note,
        ));

        // Spawn the engine on its own std thread (it runs the synchronous engine
        // loop, not a tokio task).
        let (handle, _join) = engine_actor::spawn_engine_thread(engine);

        // The shared shutdown primitive: a SIGINT/SIGTERM or a first-listener
        // failure flips its watch and every serve task awaits it.
        let shutdown = ServeShutdown::new();
        // Collect the IPs the server actually bound to (for the host allowlist).
        // Uses the bound addresses captured above, BEFORE the listeners move into
        // the serve tasks. Together with `server.allowed_hosts` from config this
        // drives the DNS-rebinding guard; loopback is always allowed regardless.
        let bound_ips: Vec<std::net::IpAddr> = bound.iter().map(|b| b.addr.ip()).collect();

        // Build ONE app, clone the router across listeners (it is a cheap
        // `Arc`-backed service). The console + access-log toggle ride into the
        // router so the WS handlers and the access middleware emit to the terminal.
        // The host allowlist is threaded in via `with_host_allowlist` so
        // `build_app` can wrap the whole router with the guard as its outermost
        // layer (outside the access log, so rejected probes are not logged).
        let app = server::build_app(
            handle.clone(),
            axum::Router::new(),
            RouterParams::plain_http()
                .with_console(console.clone(), access_log)
                .with_max_websocket_connections(max_ws_caps.0, max_ws_caps.1, max_ws_caps.2)
                .with_host_allowlist(bound_ips, engine_allowed_hosts.clone()),
        );

        // Translate a SIGINT/SIGTERM into a watch trip so every listener winds
        // down gracefully (the same trigger a first-listener failure uses).
        {
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                shutdown_signal().await;
                shutdown.trigger();
            });
        }

        // Serve every BOUND address; each serve task's graceful-shutdown future
        // awaits the shared watch, so any trip (signal OR a sibling's failure)
        // winds them all down together.
        let mut tasks = tokio::task::JoinSet::new();
        for BoundListener { listener, .. } in bound {
            let app = app.clone();
            let shutdown = shutdown.clone();
            let task_shutdown = shutdown.subscribe();
            tasks.spawn(async move {
                // Serve with connect-info so the access-log middleware can include
                // the peer IP in each log line. `tap_io` disables Nagle on
                // each accepted socket: terminal traffic is many tiny packets
                // (keystrokes, per-char echo/redraws), and Nagle batches them into
                // laggy clumps that make remote typing stutter and flicker.
                let result = axum::serve(
                    listener.tap_io(|stream| {
                        let _ = stream.set_nodelay(true);
                    }),
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(wait_for_shutdown(task_shutdown))
                .await;
                if let Err(e) = &result {
                    // The accept loop died while serving (graceful shutdown returns
                    // Ok). Record the first error and trip the watch so the OTHER
                    // listeners wind down too — never let the server limp on with
                    // one dead listener.
                    dux_core::logger::error(&format!(
                        "[server] a plain-HTTP listener's accept loop failed; \
                         shutting the server down: {e}"
                    ));
                    shutdown.record_failure(anyhow::anyhow!("web server listener failed: {e}"));
                }
                result
            });
        }
        // Wait for all serve tasks to finish (they all wind down together once the
        // watch trips). A task that PANICKED yields a JoinError here and recorded
        // nothing, so record it and trip the watch so siblings stop too.
        while let Some(joined) = tasks.join_next().await {
            if let Err(join_err) = joined {
                dux_core::logger::error(&format!(
                    "[server] a plain-HTTP serve task panicked: {join_err} — shutting the other \
                     listeners down so the server does not limp on half-dead."
                ));
                shutdown.record_failure(anyhow::anyhow!(
                    "a plain-HTTP serve task panicked: {join_err}"
                ));
            }
        }
        // SIGTERM the agents (they save state for a later resume), mark their
        // sessions Detached, then exit; Drop hard-kills any straggler.
        shutdown.trigger();
        handle.shutdown().await;
        match shutdown.take_error() {
            Some(e) => Err(e),
            None => Ok::<(), anyhow::Error>(()),
        }
    })
}

/// What the status-screen tick asks `serve_with_engine` to do after the current
/// iteration. `Continue` keeps serving; `ReturnToTui` flips back to the TUI
/// (server torn down, PTYs preserved); `QuitProcess` exits the whole process
/// (server torn down, agents SIGTERMed).
pub enum ServerTick {
    Continue,
    ReturnToTui,
    QuitProcess,
}

/// How `serve_with_engine` exited, so the binary's orchestration loop knows
/// whether to resume the TUI or quit.
pub enum ServerExit {
    ReturnToTui,
    QuitProcess,
}

/// Grace period given to agent/terminal children to flush state on a quit, the
/// same window the dedicated-thread `Shutdown` path uses.
const QUIT_PTY_GRACE: Duration = Duration::from_millis(1500);

/// Upper bound on how long the flip waits for the axum server task to finish
/// after graceful shutdown is triggered. A wedged client connection must not be
/// able to hang the flip back to the TUI, so we cap the join and tear the
/// runtime down with a bounded timeout afterward.
const SERVER_JOIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Upper bound on the runtime teardown itself. `Runtime::drop` blocks until every
/// `spawn_blocking` task returns and CANNOT abort them, so a parked blocking task
/// (e.g. a PTY forwarder still inside `recv_timeout`) would hang an implicit drop
/// forever. `shutdown_timeout` instead detaches stragglers after this window, so
/// the flip back to the TUI always proceeds. The teardown flag should already
/// have unparked the forwarders well within this bound; this is belt-and-braces.
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// The ONE serve-shutdown primitive shared by all three serve paths
/// (`run_plain_http`, `serve_with_engine`). It bundles the
/// first-error bookkeeping with the `watch<bool>` shutdown lane every listener
/// awaits, so a single dying listener winds the siblings down identically
/// everywhere:
///
/// - `failed` — armed once (compare-exchange) so the FIRST failing listener is
///   the one that records the returned error and is reported.
/// - `error` — the first error, surfaced to the caller after wind-down.
/// - `shutdown_tx` — flipped to `true` on the first failure (and on a normal
///   SIGINT/SIGTERM); every serve task's graceful-shutdown future awaits the
///   matching receiver, so tripping it stops the whole server. Replaces the old
///   `run_plain_http` no-abort JoinSet wait.
#[derive(Clone)]
struct ServeShutdown {
    failed: Arc<AtomicBool>,
    error: Arc<std::sync::Mutex<Option<anyhow::Error>>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl ServeShutdown {
    fn new() -> Self {
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        Self {
            failed: Arc::new(AtomicBool::new(false)),
            error: Arc::new(std::sync::Mutex::new(None)),
            shutdown_tx,
        }
    }

    /// A fresh receiver on the shutdown lane (one per serve task).
    fn subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    /// Whether a serve task has already recorded a failure (polled by the flip's
    /// engine-loop control closure to exit the loop).
    fn is_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// Trigger a graceful, non-error wind-down (a SIGINT/SIGTERM or the flip's
    /// engine loop returning). Idempotent.
    fn trigger(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Take the first recorded serve error, if any. Called once after every
    /// listener has wound down so the caller can surface a genuine death.
    fn take_error(&self) -> Option<anyhow::Error> {
        self.error.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Record a serve-task failure exactly once and wind the whole server down.
    ///
    /// Called by every per-listener serve task whose accept loop returns an `Err`
    /// (graceful shutdown returns `Ok`, so this only fires on a genuine death).
    /// The FIRST caller wins: it stores the error
    /// and is the one reported; later callers (other listeners winding down behind
    /// it) no-op the error slot. Always trips the shutdown watch so the remaining
    /// listeners stop too. Returns `true` when this call was the first-error
    /// winner.
    fn record_failure(&self, err: anyhow::Error) -> bool {
        let first = self
            .failed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        if first && let Ok(mut slot) = self.error.lock() {
            *slot = Some(err);
        }
        // Always wind the others down, even for a non-first caller (idempotent).
        let _ = self.shutdown_tx.send(true);
        first
    }
}

/// Await the shared shutdown lane: resolve once the watch flips to `true` (a
/// SIGINT/SIGTERM trigger or a first-listener failure). The receiver is consumed,
/// so each caller passes its own [`ServeShutdown::subscribe`] handle. A
/// wakeup-driven await (no sleep-poll).
async fn wait_for_shutdown(mut rx: tokio::sync::watch::Receiver<bool>) {
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            break;
        }
    }
}

/// Serve the web UI over an EXISTING engine on the CALLER's thread, returning
/// the engine when serving stops. This is the in-process TUI↔server flip's
/// entry point: the TUI hands its live `Engine` (PTYs running, owned on the main
/// thread) and pre-bound std `TcpListener`s here; this turns the caller's thread
/// INTO the engine-actor loop while axum serves on a background runtime. LOCAL
/// MODE may bind more than one address (loopback + the machine's Tailscale
/// address), so `listeners` is a vector and one axum task serves each, sharing
/// the router/state; graceful shutdown stops them all.
///
/// `on_tick` runs once per engine-loop iteration (the binary implements it with
/// a dux-tui status screen that polls keys and redraws). Its return value drives
/// the exit:
/// - `Continue` keeps serving.
/// - `ReturnToTui` triggers graceful axum shutdown and returns `(engine,
///   ReturnToTui)` with PTYs UNTOUCHED — the TUI resumes around the same agents.
/// - `QuitProcess` (or a SIGINT/SIGTERM during serving) triggers graceful axum
///   shutdown, then SIGTERMs the children (`shutdown_ptys`) like the CLI path,
///   and returns `(engine, QuitProcess)`.
pub fn serve_with_engine(
    mut engine: Engine,
    listeners: Vec<std::net::TcpListener>,
    activity: dux_core::activity::ActivityRing,
    mut on_tick: impl FnMut() -> ServerTick,
) -> Result<(Engine, ServerExit)> {
    // The flip owns the terminal with its themed status screen, so this console
    // writes NOTHING to stdout — but it captures every lifecycle event into the
    // shared ring that drives the status screen's Activity panel.
    let console = Console::capture(activity);
    let (handle, ends) = engine_actor::build_actor_channels(&engine);
    engine_actor::spawn_global_workers(&mut engine);

    // Grab the teardown flag before the handle moves into the router. We trip it
    // the instant the engine loop exits (before axum graceful shutdown) so any
    // PTY forwarders parked on their blocking `recv_timeout` exit within one poll
    // window — even on ReturnToTui, where the engine and its PtyClient senders
    // stay alive and the forwarders' channels would otherwise never disconnect.
    let shutdown_flag = handle.shutdown_flag();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Collect the flip's bound IPs (for the host allowlist) and the operator's
    // configured hosts. Read them HERE -- from the std TcpListeners -- before the
    // conversion loop below moves `listeners` into the tokio listener set.
    let flip_bound_ips: Vec<std::net::IpAddr> = listeners
        .iter()
        .filter_map(|l| l.local_addr().ok())
        .map(|a| a.ip())
        .collect();
    let flip_allowed_hosts = engine.config.server.allowed_hosts.clone();
    let flip_max_ws = (
        engine.config.server.max_websocket_events_connections,
        engine.config.server.max_websocket_agent_connections,
        engine.config.server.max_websocket_terminal_connections,
    );

    // The std listeners travel through the flip (the TUI bound them BEFORE tearing
    // down, so there is no rebind race); tokio needs them non-blocking. Adoption
    // failures here are rare (the bind already succeeded in the preflight), but log
    // the failing address before propagating so a flip that cannot start the server
    // leaves a forensic record in dux.log, not just a TUI status line.
    let tokio_listeners = {
        let _guard = runtime.enter();
        let mut out = Vec::with_capacity(listeners.len());
        for listener in listeners {
            let addr = listener
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|_| "<unknown address>".to_string());
            if let Err(err) = listener.set_nonblocking(true) {
                dux_core::logger::error(&format!(
                    "[server] could not adopt the pre-bound flip listener on {addr} \
                     (set_nonblocking failed): {err}"
                ));
                return Err(err.into());
            }
            match tokio::net::TcpListener::from_std(listener) {
                Ok(l) => out.push(l),
                Err(err) => {
                    dux_core::logger::error(&format!(
                        "[server] could not adopt the pre-bound flip listener on {addr} \
                         (tokio from_std failed): {err}"
                    ));
                    return Err(err.into());
                }
            }
        }
        out
    };

    // The shared shutdown primitive — the SAME [`ServeShutdown`] the CLI serve
    // paths use. Its watch is the graceful-shutdown lane every serve task and the
    // sweep await; the synchronous engine loop flips it on exit, a SIGINT/SIGTERM
    // flips it via the signal task, and a dying listener flips it via
    // `record_failure`. The control closure polls `is_failed()` so a listener
    // death also breaks the engine loop, and `take_error()` surfaces the death to
    // the caller.
    let shutdown = ServeShutdown::new();
    // Set by the signal task; polled by the control closure so a SIGINT/SIGTERM
    // received while serving breaks the engine loop too (not just axum). Distinct
    // from the failure flag because a signal means QuitProcess, a failure means
    // ReturnToTui-with-error.
    let signal_quit = Arc::new(AtomicBool::new(false));

    // Build ONE app, shared across listeners (the router is a cheap `Arc`-backed
    // service). `build_app` constructs the `ChangesService`, which spawns its
    // supervised poller via `tokio::spawn` -- that needs an entered runtime, and
    // the flip is not yet inside `block_on` here, so enter the runtime for the
    // build.
    let app = {
        let _guard = runtime.enter();
        server::build_app(
            handle.clone(),
            axum::Router::new(),
            RouterParams::plain_http()
                // The capture console keeps the access log OFF (it is never wanted
                // in the panel, and access() never reaches emit() to be captured
                // anyway) while the WS handlers feed lifecycle events into the ring.
                .with_console(console.clone(), false)
                .with_max_websocket_connections(flip_max_ws.0, flip_max_ws.1, flip_max_ws.2)
                .with_host_allowlist(flip_bound_ips, flip_allowed_hosts),
        )
    };

    // One axum serve task per listener, all sharing the same router/state and the
    // same shutdown watch. A JoinSet lets us join them all (bounded) on teardown.
    let mut server_tasks = tokio::task::JoinSet::new();
    for tokio_listener in tokio_listeners {
        let app = app.clone();
        let shutdown = shutdown.clone();
        let task_shutdown = shutdown.subscribe();
        server_tasks.spawn_on(
            async move {
                // Disable Nagle per accepted socket (see run_plain_http) so
                // interactive terminal traffic isn't batched into laggy clumps.
                let result = axum::serve(
                    tokio_listener.tap_io(|stream| {
                        let _ = stream.set_nodelay(true);
                    }),
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(wait_for_shutdown(task_shutdown))
                .await;
                if let Err(err) = &result {
                    // The accept loop died while serving (graceful shutdown returns
                    // Ok). Log loudly, then record the first error and trip the
                    // watch (so the engine loop exits via `is_failed()` and the
                    // OTHER listeners wind down too) — never let the server limp on
                    // with one dead listener.
                    dux_core::logger::error(&format!(
                        "[server] a listener's accept loop failed; shutting the flip down: {err}"
                    ));
                    shutdown.record_failure(anyhow::anyhow!("web server listener failed: {err}"));
                }
                result
            },
            runtime.handle(),
        );
    }
    // The router holds its own cloned handle(s); drop ours so only the serve
    // tasks keep the request side alive (matches the pre-multi-listener move).
    drop(handle);

    // Signal task: trip the flag on SIGINT/SIGTERM so the control closure exits
    // the loop with QuitProcess on the next tick.
    let signal_flag = Arc::clone(&signal_quit);
    runtime.spawn(async move {
        shutdown_signal().await;
        signal_flag.store(true, Ordering::SeqCst);
    });

    // Run the engine loop on the CURRENT thread. The control closure decides the
    // exit reason: a serve failure or a tripped signal flag wins (both exit the
    // loop), otherwise the caller's tick result maps straight through.
    let mut exit = ServerExit::ReturnToTui;
    let mut engine = engine_actor::run_engine_loop(engine, ends, || {
        if shutdown.is_failed() {
            // A listener died: exit the loop. We RETURN to the TUI rather than
            // quit the process (PTYs stay intact) and surface the captured error
            // below so the caller knows the server could not keep serving.
            exit = ServerExit::ReturnToTui;
            return LoopControl::Exit;
        }
        if signal_quit.load(Ordering::SeqCst) {
            exit = ServerExit::QuitProcess;
            return LoopControl::Exit;
        }
        match on_tick() {
            ServerTick::Continue => LoopControl::Continue,
            ServerTick::ReturnToTui => {
                exit = ServerExit::ReturnToTui;
                LoopControl::Exit
            }
            ServerTick::QuitProcess => {
                exit = ServerExit::QuitProcess;
                LoopControl::Exit
            }
        }
    });

    // The engine loop has returned. Trip the teardown flag FIRST so any PTY
    // forwarders parked on their blocking `recv_timeout` exit within one poll
    // window — the engine (and its PtyClient senders) is still alive on
    // ReturnToTui, so the forwarders' channels never disconnect on their own.
    // Without this, `Runtime::shutdown_timeout` below would block until the flag
    // window elapses (and an implicit drop would hang forever).
    shutdown_flag.store(true, Ordering::SeqCst);

    // Trigger graceful axum shutdown and wait (bounded) for ALL server tasks to
    // wind down. A single bounded join over the whole set keeps a wedged client
    // connection on any listener from hanging the flip back to the TUI.
    shutdown.trigger();
    runtime.block_on(async {
        let _ = tokio::time::timeout(SERVER_JOIN_TIMEOUT, async {
            while server_tasks.join_next().await.is_some() {}
        })
        .await;
    });
    // Tear the runtime down with a bounded timeout. An implicit `drop(runtime)`
    // would block forever on any parked `spawn_blocking` task (drop cannot abort
    // them); `shutdown_timeout` detaches stragglers instead, so the flip cannot
    // wedge even if a forwarder were somehow still blocked.
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);

    if matches!(exit, ServerExit::QuitProcess) {
        // Quit teardown: SIGTERM the children so CLIs can save state for a later
        // resume, mark agent sessions Detached. We own the engine here, so we
        // call `shutdown_ptys` directly (the dedicated-thread path routes the
        // equivalent through the `Shutdown` request).
        engine.shutdown_ptys(QUIT_PTY_GRACE);
    }
    // ReturnToTui intentionally leaves PTYs untouched so the resumed TUI finds
    // the same live agents.
    //
    // We deliberately do NOT reset SIGINT/SIGTERM to SIG_DFL here. tokio's unix
    // signal support and the TUI both register through the same process-global
    // `signal-hook-registry`, which installs its master OS handler exactly once
    // per signal (at the TUI's first registration) and never re-arms it on later
    // register/unregister. The resumed TUI re-registers its own SIGINT/SIGTERM
    // handlers (`App::register_signal_handles`, always called from `App::resume`)
    // so the still-installed master handler routes the next signal to the TUI's
    // graceful-shutdown flag. Forcing the disposition back to SIG_DFL with raw
    // `libc::signal` would point the OS away from the master handler, and because
    // registry won't re-`sigaction`, the TUI's re-registration could not re-arm
    // it: an external `kill` post-flip would then terminate hard instead of
    // winding the agents down. The earlier "unkillable resumed TUI" this reset
    // once guarded against can no longer occur: the TUI now always installs a
    // terminating handler on resume. (tokio's stale per-runtime action lingers in
    // the registry across flips but is a harmless no-op once its runtime drops.)

    // If a listener's accept loop died (F5), surface the captured error rather
    // than reporting a clean exit. The engine has already been wound down above,
    // so the caller drops it; the TUI shows the failure instead of resuming onto
    // a server that silently stopped serving.
    if let Some(err) = shutdown.take_error() {
        return Err(err);
    }

    Ok((engine, exit))
}

/// Resolves when the process receives SIGINT (Ctrl-C) or SIGTERM. The first such
/// signal resolves this future — the caller then triggers a graceful shutdown —
/// and also arms a watcher so a SECOND signal forces an immediate exit, rather
/// than leaving the operator trapped if the graceful drain wedges.
async fn shutdown_signal() {
    // Install both handlers ONCE up front and reuse the same streams for the
    // first wait AND the second-signal force-quit watcher. Re-subscribing fresh
    // after the first signal fired would race: a rapid second signal could arrive
    // in the window before a newly-created listener is registered and be missed.
    // A persistent `Signal` stream stays armed and catches the next delivery
    // whenever it is next polled.
    let mut interrupt = install_signal(
        tokio::signal::unix::SignalKind::interrupt(),
        "SIGINT (Ctrl-C)",
    );
    let mut terminate = install_signal(tokio::signal::unix::SignalKind::terminate(), "SIGTERM");

    if interrupt.is_none() && terminate.is_none() {
        // Neither handler installed — we can observe no stop signal. Park so this
        // future never resolves spuriously; `install_signal` already logged loudly.
        std::future::pending::<()>().await;
    }

    next_terminate_signal(&mut interrupt, &mut terminate).await;

    // A graceful shutdown has now been requested. If it wedges — a stuck PTY
    // write, a client socket that never closes, an unbounded connection drain — a
    // SECOND Ctrl-C/SIGTERM must NOT be swallowed, or the operator is trapped and
    // forced to `kill -9`. Reuse the already-armed streams (so there is no
    // re-registration gap) and force-exit on the next signal. This deliberately
    // bypasses the (possibly stuck) graceful path: the "I really mean stop" escape
    // hatch, mirroring how most servers treat a second Ctrl-C. 130 = 128 + SIGINT,
    // the conventional interrupted-exit code.
    tokio::spawn(async move {
        next_terminate_signal(&mut interrupt, &mut terminate).await;
        let msg = "[server] second interrupt received during shutdown — forcing immediate exit.";
        dux_core::logger::error(msg);
        eprintln!("{msg}");
        std::process::exit(130);
    });
}

/// Install a SIGINT/SIGTERM handler, returning the stream — or `None` (logged
/// loudly) if registration fails, so the caller can still rely on the other
/// signal. `label` is the human name used in the failure message.
fn install_signal(
    kind: tokio::signal::unix::SignalKind,
    label: &str,
) -> Option<tokio::signal::unix::Signal> {
    match tokio::signal::unix::signal(kind) {
        Ok(sig) => Some(sig),
        Err(e) => {
            // Registering this handler failed: say so loudly instead of dropping
            // the error. The other signal still gives a graceful stop; if BOTH
            // fail, `shutdown_signal` parks rather than firing spuriously.
            let msg = format!(
                "[server] failed to install the {label} handler: {e} — {label} will not stop the \
                 server; rely on the other signal (Ctrl-C for SIGINT, systemctl/docker stop for \
                 SIGTERM)."
            );
            dux_core::logger::error(&msg);
            eprintln!("ERROR: {msg}");
            None
        }
    }
}

/// Await the next delivery of either signal stream. A stream that failed to
/// install (`None`) is treated as never-firing so the other still works.
async fn next_terminate_signal(
    interrupt: &mut Option<tokio::signal::unix::Signal>,
    terminate: &mut Option<tokio::signal::unix::Signal>,
) {
    async fn recv(sig: &mut Option<tokio::signal::unix::Signal>) {
        match sig {
            Some(s) => {
                // `recv()` yields `None` only when the stream closes (runtime
                // teardown), which is NOT a delivered signal — resolving on it
                // would make the second-signal watcher force-exit spuriously
                // during a clean shutdown. Park on a closed stream so this arm
                // never fires (and so we don't busy-loop on a persistent `None`).
                if s.recv().await.is_none() {
                    std::future::pending::<()>().await;
                }
            }
            None => std::future::pending::<()>().await,
        }
    }
    tokio::select! {
        _ = recv(interrupt) => {},
        _ = recv(terminate) => {},
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Reachability, ServeShutdown, bind_plan_addrs, plain_http_banner, reachability, safety_note,
        tailscale_bind_warning, wait_for_shutdown,
    };
    use dux_core::config::PlanAddr;
    use dux_core::engine::Command;

    #[test]
    fn flip_console_captures_into_the_shared_ring() {
        // The flip path builds its console from the shared ring; a client-connect
        // event on that console must land in the ring the status screen reads.
        let ring = dux_core::activity::ActivityRing::new();
        let console = crate::console::Console::capture(ring.clone());
        console.client_connected("10.0.0.7".parse().unwrap());
        assert_eq!(ring.connections(), 1);
        assert_eq!(
            ring.snapshot(dux_core::activity::ACTIVITY_CAP).events.len(),
            1
        );
    }

    #[test]
    fn tailscale_bind_warning_names_addr_cause_and_both_remedies() {
        // The warning must name the busy address, the cause, and BOTH remedies
        // (stop the other process, or change the port) so an operator can act.
        let addr = "100.64.0.1:8080".parse().unwrap();
        let err = std::io::Error::new(std::io::ErrorKind::AddrInUse, "address already in use");
        let w = tailscale_bind_warning(addr, &err);
        assert!(w.contains("100.64.0.1:8080"), "must name the address: {w}");
        assert!(
            w.contains("address already in use"),
            "must name the cause: {w}"
        );
        assert!(
            w.contains("Stop that process"),
            "must offer the stop-the-process remedy: {w}"
        );
        assert!(
            w.contains("[server].port"),
            "must offer the change-the-port remedy: {w}"
        );
    }

    #[tokio::test]
    async fn bind_plan_addrs_drops_best_effort_failure_and_keeps_required() {
        // The real-world bug: a third-party process holds the best-effort
        // (Tailscale) address while the required (loopback) address is free. The
        // bind must SUCCEED on the required leg, DROP the failed best-effort leg,
        // and return a warning naming it. host-only-from-bound is the caller's
        // concern; here we prove the bound set excludes the failed address.
        //
        // 127.0.0.2 stands in for the Tailscale IP (all of 127.0.0.0/8 is loopback
        // on Linux), held on an ephemeral port; 127.0.0.1 on a separate free port
        // is the required leg. The bind-failure path doesn't care that it's not a
        // real Tailscale address — only that the entry is best-effort.
        let held = std::net::TcpListener::bind("127.0.0.2:0").expect("hold a best-effort addr");
        let held_addr = held.local_addr().expect("held addr");
        let free = std::net::TcpListener::bind("127.0.0.1:0").expect("probe a free port");
        let required_addr = free.local_addr().expect("free addr");
        drop(free); // release it so bind_plan_addrs can take it

        let plan = vec![
            PlanAddr::required(required_addr),
            PlanAddr::best_effort(held_addr),
        ];
        let (bound, warnings) = bind_plan_addrs(&plan)
            .await
            .expect("a busy best-effort leg must not fail the serve");

        assert_eq!(bound.len(), 1, "only the required leg binds");
        assert_eq!(
            bound[0].addr, required_addr,
            "the bound leg is the required one"
        );
        assert!(
            bound.iter().all(|b| b.addr.ip().is_loopback()),
            "every bound addr is loopback → host-only"
        );
        assert_eq!(
            warnings.len(),
            1,
            "exactly one best-effort warning: {warnings:?}"
        );
        assert!(
            warnings[0].contains(&held_addr.to_string()),
            "the warning names the busy best-effort address: {}",
            warnings[0]
        );
    }

    #[tokio::test]
    async fn bind_plan_addrs_required_failure_is_fatal_and_names_the_addr() {
        // A REQUIRED address that is already held must FAIL the whole bind with the
        // address in the error message (the explicit-failure tenet — the operator
        // named this address). dux.log also gets a logger::error (not asserted here
        // because the test logger is process-global; the message text is the
        // contract we pin).
        let held = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a required addr");
        let held_addr = held.local_addr().expect("held addr");

        let plan = vec![PlanAddr::required(held_addr)];
        let err = bind_plan_addrs(&plan)
            .await
            .expect_err("a busy required address must be fatal");
        let text = format!("{err:#}");
        assert!(
            text.contains("could not bind the listen address")
                && text.contains(&held_addr.to_string()),
            "the fatal error must name the busy required address: {text}"
        );
    }

    // ── Startup banner builders ────────────────────────────────────────────

    fn addr(s: &str) -> std::net::SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn reachability_worst_wins_across_legs() {
        // Loopback-only.
        assert_eq!(
            reachability(&[(addr("127.0.0.1:8080"), true)]),
            Reachability::LoopbackOnly
        );
        // A best-effort non-loopback leg → Tailscale.
        assert_eq!(
            reachability(&[
                (addr("127.0.0.1:8080"), true),
                (addr("100.64.0.5:8080"), false)
            ]),
            Reachability::Tailscale
        );
        // A required non-loopback leg wins over a Tailscale one (worst-wins).
        assert_eq!(
            reachability(&[
                (addr("100.64.0.5:8080"), false),
                (addr("0.0.0.0:8080"), true)
            ]),
            Reachability::Public
        );
        // Empty (vacuously loopback-only).
        assert_eq!(reachability(&[]), Reachability::LoopbackOnly);
    }

    // ── safety_note ───────────────────────────────────────────────────────────

    fn plan_addr(s: &str, required: bool) -> PlanAddr {
        if required {
            PlanAddr::required(s.parse().unwrap())
        } else {
            PlanAddr::best_effort(s.parse().unwrap())
        }
    }

    #[test]
    fn safety_note_loopback_only_is_none() {
        let addrs = vec![plan_addr("127.0.0.1:8080", true)];
        assert_eq!(safety_note(&addrs), None);
    }

    #[test]
    fn safety_note_loopback_plus_tailscale_mentions_tailnet() {
        let addrs = vec![
            plan_addr("127.0.0.1:8080", true),
            plan_addr("100.64.0.5:8080", false),
        ];
        let note = safety_note(&addrs).expect("must have a note for tailscale leg");
        assert!(note.contains("tailnet"), "must mention tailnet: {note}");
        assert!(
            !note.contains("NO login"),
            "tailscale note must NOT say NO login: {note}"
        );
    }

    #[test]
    fn safety_note_wildcard_primary_mentions_no_login() {
        let addrs = vec![plan_addr("0.0.0.0:8080", true)];
        let note = safety_note(&addrs).expect("must warn for 0.0.0.0");
        assert!(note.contains("NO login"), "must contain 'NO login': {note}");
    }

    #[test]
    fn safety_note_lan_primary_with_tailscale_leg_mentions_both() {
        // Overlap case: non-loopback required primary AND a Tailscale best-effort leg.
        // LAN warning wins (severity), and appends the Tailscale parenthetical.
        let addrs = vec![
            plan_addr("192.168.1.5:8080", true),
            plan_addr("100.64.0.5:8080", false),
        ];
        let note = safety_note(&addrs).expect("must warn for LAN primary");
        assert!(note.contains("NO login"), "must contain 'NO login': {note}");
        assert!(
            note.contains("Tailscale address is bound too"),
            "must note the tailscale leg: {note}"
        );
    }

    #[test]
    fn plain_http_banner_labels_loopback_tailscale_and_public_legs() {
        let legs = vec![
            (addr("127.0.0.1:8080"), true),   // loopback (required)
            (addr("100.64.0.5:8080"), false), // best-effort → Tailscale
            (addr("203.0.113.7:8080"), true), // required non-loopback → Listen
        ];
        let banner = plain_http_banner("0.1.0", &legs, &[], None);
        assert_eq!(banner.mode, "plain HTTP");
        assert_eq!(banner.listeners.len(), 3);
        assert_eq!(banner.listeners[0].label, "Local (loopback)");
        assert_eq!(banner.listeners[0].url, "http://127.0.0.1:8080");
        assert_eq!(banner.listeners[1].label, "Tailscale");
        assert_eq!(banner.listeners[2].label, "Listen");
    }

    #[test]
    fn plain_http_banner_carries_degradation_warnings() {
        let legs = vec![(addr("127.0.0.1:8080"), true)];
        let warnings = vec!["Tailscale: 100.64.0.1:8080 busy -- serving without it".to_string()];
        let banner = plain_http_banner("0.1.0", &legs, &warnings, None);
        assert_eq!(banner.warnings, warnings);
    }

    #[test]
    fn record_serve_failure_first_caller_wins_and_triggers_shutdown() {
        // The first serve task to die records its error, arms the flag, and trips
        // the shutdown watch; a later caller (another listener winding down) does
        // NOT overwrite the first error but STILL nudges shutdown. This is the F5
        // load-bearing logic, tested directly because forcing a real axum accept
        // loop to error mid-serve is inherently flaky. Now exercised through the
        // ONE shared [`ServeShutdown`] primitive every serve path uses.
        let shutdown = ServeShutdown::new();
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
        let shutdown = ServeShutdown::new();
        let waiter = shutdown.subscribe();
        shutdown.trigger();
        // Resolves promptly (bounded so a regression fails rather than hangs).
        tokio::time::timeout(std::time::Duration::from_secs(1), wait_for_shutdown(waiter))
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
        // serve future resolves (Ok — graceful), proving one listener's death
        // winds the others down. This is the run_plain_http first-error behavior
        // exercised over a real accept loop (cheap, deterministic — no flaky
        // mid-serve error injection needed: we trip the lane the sibling would).
        let shutdown = ServeShutdown::new();
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

        let joined = tokio::time::timeout(std::time::Duration::from_secs(2), serve)
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

    #[test]
    fn dux_core_command_is_constructible() {
        let cmd = Command::OpenPath {
            path: std::path::PathBuf::from("/tmp/dux-web-smoke"),
            target: "session worktree".to_string(),
        };
        // Exercise pattern-matching so the variant fields are actually
        // referenced — a dead-code construction wouldn't catch API drift.
        match cmd {
            Command::OpenPath { path, target } => {
                assert_eq!(target, "session worktree");
                assert_eq!(path.display().to_string(), "/tmp/dux-web-smoke");
            }
            _ => unreachable!("constructed an OpenPath variant"),
        }
    }
}

#[cfg(test)]
mod config_surface_tests {
    use dux_core::config::{Config, DuxPaths};
    use dux_core::engine::{ConfigSurface, ReloadCompletionGuard};
    use dux_core::worker::WorkerEvent;
    use std::sync::mpsc::{self, Sender};

    /// Minimal web-layer `ConfigSurface`: reload re-reads config (here a default)
    /// and posts `ConfigReloadReady`; recover_render produces a plain config text.
    struct WebConfigSurface;

    impl ConfigSurface for WebConfigSurface {
        fn reload(&self, _paths: DuxPaths, worker_tx: Sender<WorkerEvent>) {
            // Drive completion through the guard, matching the production surfaces
            // so the test exercises the F5-safe path rather than a bare send.
            ReloadCompletionGuard::new(worker_tx).complete(Ok(Config::default()));
        }

        fn recover_render(&self, config: &Config) -> String {
            dux_core::config_write::render_config_plain(config)
        }
    }

    /// Proves the web layer can implement `ConfigSurface` against `dux-core`
    /// alone (no TUI deps).
    #[test]
    fn web_can_implement_config_surface() {
        let (tx, rx) = mpsc::channel();
        let surface: Box<dyn ConfigSurface> = Box::new(WebConfigSurface);
        surface.reload(
            DuxPaths {
                root: std::path::PathBuf::from("/tmp/dux-web-test"),
                config_path: std::path::PathBuf::from("/tmp/dux-web-test/config.toml"),
                sessions_db_path: std::path::PathBuf::from("/tmp/dux-web-test/sessions.sqlite3"),
                worktrees_root: std::path::PathBuf::from("/tmp/dux-web-test/worktrees"),
                lock_path: std::path::PathBuf::from("/tmp/dux-web-test/dux.lock"),
            },
            tx,
        );
        let event = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("event");
        assert!(matches!(event, WorkerEvent::ConfigReloadReady(_)));

        // recover_render produces structured plain config text.
        let body = surface.recover_render(&Config::default());
        assert!(
            body.contains("[defaults]"),
            "render missing defaults: {body}"
        );
    }
}
