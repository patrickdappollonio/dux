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

pub mod background;
pub mod bootstrap;
pub mod bootstrap_routes;
pub mod browse_routes;
pub mod build_routes;
pub mod changes;
pub mod changes_routes;
pub mod compressible_exts;
pub mod config_routes;
pub mod console;
pub mod engine_actor;
pub mod event_bus;
pub mod file_drop_routes;
pub mod file_routes;
pub mod first_load_routes;
pub mod git_routes;
pub mod host_guard;
pub(crate) mod ownership_publish;
pub mod project_actions;
pub mod project_reads;
/// The PTY input-ownership registry, re-exported from `dux-core`: a rule two
/// surfaces obey belongs in the crate both can see, and the re-export keeps
/// every call site and test in this crate on the path it already uses.
///
/// A glob rather than a named list because some names are used only by this
/// crate's test modules, and a named re-export of those is an unused import in
/// a normal build.
pub(crate) mod pty_owners {
    pub(crate) use dux_core::pty_owners::*;
}
pub(crate) mod pty_sizes;
pub mod resource_routes;
pub mod rest_common;
pub mod serve_legs;
pub mod server;
pub mod session_actions;
pub mod startup_logs;
pub mod tab_actions;
pub mod terminal_actions;
pub mod web_assets;
pub mod workspace_routes;

/// Crate-wide test helpers shared by the per-module route test suites (a single
/// headless engine handle + a plain router builder), so each REST route module
/// can exercise its handlers without duplicating the bootstrap recipe.
#[cfg(test)]
pub(crate) mod test_support;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use axum::Router;
use axum::serve::ListenerExt;
use dux_core::config::{DuxPaths, PlanAddr, ServerPlan, TailscaleMode, TailscaleModeOutcome};
use dux_core::engine::Engine;
use dux_core::tailscale::TailscaleUnavailable;

use crate::console::{Banner, Console, ListenerRow};
use crate::engine_actor::LoopControl;
use crate::serve_legs::{
    LegCommand, LegStep, ModeStep, ServeShutdown, StartupLeg, TailscaleModeControl, WATCH_PERIOD,
    desired_leg, plan_leg_step, plan_mode_change, wait_for_leg_shutdown, waiting_note,
    watch_tailscale_leg,
};
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
    run_plain_http(paths, plan, version)
}

/// Log a WARN when this binary has no web UI compiled in (built with
/// `DUX_DISABLE_UI_BUILD` and no previously built `web/dist`).
///
/// The message lands in THREE places on purpose, because the two audiences are
/// different people who look in different places:
/// - the served page itself (build.rs's notice page), for whoever opens a browser,
///   possibly on a phone, with no access to this terminal;
/// - the `dux server` startup banner (a ⚠ row), for the operator who launched it
///   and can rebuild;
/// - `dux.log`, which is the ONLY one of the three the TUI flip path reaches: the
///   flip keeps its themed status screen and must not print to stdout, so it has
///   no banner to carry the row.
///
/// Called by both serve entry points so neither can forget it.
///
/// A binary that reports a real build but embeds almost nothing is warned about
/// here too, through the same row: the build state alone cannot see what
/// rust-embed baked in, and a 404 at the root with nothing said anywhere is the
/// symptom that motivated the check (see `web_assets::UI_EMPTY_EMBED_WARNING`).
fn warn_if_ui_not_built() {
    if let Some(warning) = web_assets::ui_startup_warning() {
        dux_core::logger::warn(&format!("[server] {warning}"));
    }
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

/// Build the plain-HTTP startup banner from the BOUND legs (each an
/// `(addr, required)` pair). Each leg is labeled by what it is:
/// - loopback → "Local (loopback)"
/// - a best-effort (LOCAL MODE Tailscale) leg → "Tailscale"
/// - a required non-loopback leg (an explicit `--bind` public/LAN entry) →
///   "Listen"
///
/// Best-effort bind degradations (a busy Tailscale address) become ⚠ rows, and so
/// does a binary built with `DUX_DISABLE_UI_BUILD` (`ui_warning`): the operator
/// who launched the server is the one who can rebuild it, and they may never open
/// a browser. It is listed FIRST because "the web UI in here is not what you
/// think" outranks any per-address degradation.
///
/// The parameter is the MESSAGE rather than a bool because there is more than one
/// of them (see `web_assets::ui_startup_warning`): the notice-page binary has no
/// web UI at all, a binary that reused an existing `web/dist` serves a real one of
/// unknown age, and a binary whose embed came out empty despite a real build
/// serves nothing while claiming otherwise. A bool could only pick one of those
/// and would be wrong the rest of the time. Pure (over `(SocketAddr, bool)` pairs and an `Option<&str>`,
/// not the live listeners or the compiled-in markers) so it is unit-testable
/// without binding sockets or rebuilding.
fn plain_http_banner(
    version: &str,
    bound: &[(SocketAddr, bool)],
    bind_warnings: &[String],
    security_note: Option<String>,
    ui_warning: Option<&'static str>,
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
    let mut warnings = Vec::with_capacity(bind_warnings.len() + 1);
    if let Some(ui_warning) = ui_warning {
        warnings.push(ui_warning.to_string());
    }
    warnings.extend(bind_warnings.iter().cloned());
    Banner {
        version: version.to_string(),
        mode: "plain HTTP".to_string(),
        warnings,
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
pub const SAFETY_NOTE_TAILNET: &str = "Reachable by other devices on your tailnet whenever this machine is connected to it \
     (no login). Set tailscale = \"no\" under [server] to serve without that address.";

/// The tailnet safety note for a serve that is WATCHING the interface (the `auto`
/// mode) rather than holding a Tailscale listener right now.
///
/// A separate sentence because the plain note would be a lie in both directions:
/// dux is not reachable on the tailnet at this instant, and it is not going to
/// stay unreachable either. The note has to cover the whole run, because it is
/// printed once and the leg comes and goes behind it.
pub const SAFETY_NOTE_TAILNET_WATCHED: &str = "Reachable by other devices on your tailnet whenever this machine is connected to it \
     (no login), including after a reconnect: dux binds your Tailscale address by itself \
     when the interface appears. Set tailscale = \"no\" under [server] to serve without it.";

/// Safety note shown when the server is bound on a required non-loopback
/// (public/LAN) address. Exported alongside [`SAFETY_NOTE_TAILNET`] so both
/// operator-facing strings live in one place.
pub const SAFETY_NOTE_PUBLIC: &str = "Reachable on your network with NO login. \
     Anyone who can reach this address controls your agents and worktrees. \
     Put it behind Tailscale or a trusted reverse proxy.";

/// Suffix appended to [`SAFETY_NOTE_PUBLIC`] when a Tailscale best-effort leg
/// is ALSO bound alongside the required public/LAN primary.
pub const SAFETY_NOTE_TAILSCALE_ALSO_BOUND: &str = " (The Tailscale address is bound too.)";

/// Operator-facing safety note based on the bound addresses' reachability and
/// the Tailscale mode. Returns None when the server is loopback-only AND cannot
/// grow a tailnet leg later (nothing to warn about).
///
/// Uses highest-severity-wins: a required non-loopback primary yields the LAN
/// warning regardless of whether a Tailscale leg is also bound.
///
/// The mode matters because this note is computed ONCE and the Tailscale leg
/// comes and goes behind it on `auto`. A loopback-only serve that is watching the
/// interface is a serve that will be reachable on the tailnet the moment the
/// laptop reconnects, and saying nothing would be the wrong half of the truth.
pub fn safety_note(addrs: &[PlanAddr], tailscale: TailscaleMode) -> Option<String> {
    let pairs: Vec<(SocketAddr, bool)> =
        addrs.iter().map(|a| (a.addr(), a.is_required())).collect();
    match reachability(&pairs) {
        Reachability::LoopbackOnly if tailscale.watches_interface() => {
            Some(SAFETY_NOTE_TAILNET_WATCHED.to_string())
        }
        Reachability::LoopbackOnly => None,
        Reachability::Tailscale if tailscale.watches_interface() => {
            Some(SAFETY_NOTE_TAILNET_WATCHED.to_string())
        }
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

/// The plain-HTTP serve path: one leg per listener (loopback, Tailscale, LAN, or
/// proxy-fronted), sharing the router/state, plus the Tailscale watcher on the
/// `auto` mode. Shutdown rides the [`ServeShutdown`] lanes: a SIGINT/SIGTERM or a
/// required leg's death trips the parent lane, which fans out over every leg, so
/// the siblings get a graceful shutdown and the error propagates.
///
/// A BEST-EFFORT (Tailscale) address whose bind fails (a third-party process
/// already holds it) does NOT abort the serve: it warns loudly to `dux.log` and
/// the server keeps serving the remaining addresses. Startup bind warnings are NOT
/// re-broadcast to web clients, because the status broadcast has no replay and
/// clients only subscribe when their WS connects, which is always after this
/// startup bind, so a startup broadcast would reach zero receivers. `dux.log` and
/// the startup banner are the delivery surfaces here; MID-RUN leg changes (the
/// watcher's doing) go to `dux.log` and the console, which is this terminal for
/// `dux server` and the flip status screen's activity panel for the flip.
fn run_plain_http(paths: DuxPaths, plan: ServerPlan, version: String) -> Result<()> {
    let ServerPlan {
        addrs,
        primary,
        tailscale,
        forced_no,
    } = plan;
    warn_if_ui_not_built();
    let engine = bootstrap::bootstrap_engine(&paths)?;
    // Build the vite-style CLI console (color from [server] color) + the access-log
    // toggle before the engine moves into the actor thread.
    let (console, access_log) = build_console(&engine.config);
    // The router's whole `[server]` input, snapshotted before the engine moves
    // into the actor thread. One clone rather than a field-by-field capture, so
    // this path and the two in-app ones derive their router from the same
    // `router_params` recipe and a new limit cannot reach two of the three.
    let router_config = engine.config.clone();
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
        let initial_tailscale_leg = bound
            .iter()
            .find(|b| !b.required && !b.addr.ip().is_loopback())
            .map(|b| b.addr);
        // Detection outcome versus bind outcome: the PLAN carries the first (a
        // best-effort address is only in it when an address was detected), `bound`
        // carries the second. The note has to tell them apart, because "dux is
        // waiting for the interface" is false when the interface is up and it was
        // the bind that failed.
        let startup_leg = match (
            addrs.iter().any(|p| !p.is_required()),
            initial_tailscale_leg,
        ) {
            (_, Some(_)) => StartupLeg::Bound,
            (true, None) => StartupLeg::BindFailed,
            (false, None) => StartupLeg::Undetected,
        };
        let note = safety_note(&bound_plan_addrs, tailscale);
        // The third state the banner has to be able to say: on `auto` with no
        // address yet, dux is not "without Tailscale", it is waiting for it.
        let mut banner_warnings = bind_warnings.clone();
        banner_warnings.extend(waiting_note(tailscale, startup_leg));
        console.banner(&plain_http_banner(
            &version,
            &banner_legs,
            &banner_warnings,
            note,
            web_assets::ui_startup_warning(),
        ));

        // Spawn the engine on its own std thread (it runs the synchronous engine
        // loop, not a tokio task).
        let (handle, _join) = engine_actor::spawn_engine_thread(engine);

        // The shared shutdown primitive: a SIGINT/SIGTERM or a first-listener
        // failure flips its watch and every serve task awaits it. It carries the
        // mode's watched-ness so a dying Tailscale leg can say truthfully whether
        // anything is going to bind it again.
        // The live-mode collaborators, created BEFORE the router so the Host
        // guard can read the mode from the same cell the serve loop writes.
        let (mode_control, mode_requests) = TailscaleModeControl::new(
            tokio::runtime::Handle::current(),
            Arc::new(AtomicBool::new(tailscale.watches_interface())),
            Arc::new(AtomicBool::new(tailscale.wants_tailscale())),
        );
        handle.set_tailscale_mode_control(mode_control.clone());
        let shutdown = ServeShutdown::new(mode_control.watched());
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
            router_params(
                &router_config,
                console.clone(),
                access_log,
                bound_ips,
                BackgroundHooks::default(),
            )
            // `router_params` derives rule 5 from the CONFIGURED mode; this run
            // serves under the EFFECTIVE one, so `--no-tailscale` would otherwise
            // leave the guard admitting tailnet literals. The live cell is what
            // the guard actually reads, and it starts from the effective mode.
            .with_tailscale_host_literals(tailscale.wants_tailscale())
            .with_live_tailscale_host_literals(mode_control.host_literals())
            .with_tailscale_mode_control(mode_control.clone(), forced_no),
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

        // Serve every BOUND address, each on its own leg (its own stop lane), so
        // the Tailscale leg can be added and dropped later without disturbing the
        // required one.
        let mut tasks = tokio::task::JoinSet::new();
        for BoundListener {
            listener,
            addr,
            required,
        } in bound
        {
            spawn_leg(
                &mut tasks,
                app.clone(),
                listener,
                addr,
                required,
                &shutdown,
                console.clone(),
            );
        }

        // On `auto`, watch the Tailscale interface for the rest of the run.
        let mut tailscale_loop = TailscaleLoop::new(
            tailscale,
            forced_no,
            Some(primary),
            initial_tailscale_leg,
            &mode_control,
            Arc::new(dux_core::tailscale::detect_ip),
        );
        let leg_commands = tailscale_loop.take_leg_receiver();
        tailscale_loop.start_watcher_if_wanted();

        run_serve_loop(
            tasks,
            shutdown.clone(),
            leg_commands,
            mode_requests,
            app,
            console.clone(),
            tailscale_loop,
        )
        .await;
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

/// Depth of the watcher-to-serve-loop command channel. A transition every ten
/// seconds at the very most, so anything above a couple of slots is theatre; the
/// bound exists so a wedged serve loop cannot make the watcher grow memory.
const LEG_COMMAND_QUEUE: usize = 8;

/// Spawn one listener's serve task into `tasks`, registering its per-leg stop
/// lane so it can be stopped on its own. The task's graceful-shutdown future
/// waits on BOTH that lane and the parent's, so a per-leg stop and a whole-server
/// teardown both reach it.
///
/// A REQUIRED leg's accept-loop death is fatal for the serve; a BEST-EFFORT leg's
/// is logged and isolated. That split is the whole reason legs exist.
fn spawn_leg(
    tasks: &mut tokio::task::JoinSet<()>,
    app: Router,
    listener: tokio::net::TcpListener,
    addr: SocketAddr,
    required: bool,
    shutdown: &ServeShutdown,
    console: Console,
) {
    let leg_lane = shutdown.register_leg(addr);
    let parent_lane = shutdown.subscribe();
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        // Serve with connect-info so the access-log middleware can include the
        // peer IP in each log line. `tap_io` disables Nagle on each accepted
        // socket: terminal traffic is many tiny packets (keystrokes, per-char
        // echo/redraws), and Nagle batches them into laggy clumps that make
        // remote typing stutter and flicker.
        let result = axum::serve(
            listener.tap_io(|stream| {
                let _ = stream.set_nodelay(true);
            }),
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(wait_for_leg_shutdown(parent_lane, leg_lane))
        .await;
        match (&result, required) {
            (Ok(()), _) => shutdown.forget_leg(addr),
            (Err(err), true) => {
                // The accept loop died while serving (graceful shutdown returns
                // Ok). Record the first error and trip the parent so the OTHER
                // listeners wind down too: never let the server limp on with one
                // dead required listener.
                dux_core::logger::error(&format!(
                    "[server] the listener on {addr} failed; shutting the server down: {err}"
                ));
                shutdown.record_failure(anyhow::anyhow!("web server listener failed: {err}"));
            }
            (Err(err), false) => {
                let err = anyhow::anyhow!("{err}");
                shutdown.record_best_effort_failure(addr, &err);
                console.bind_degraded(&format!(
                    "the Tailscale listener on {addr} stopped serving: {err}"
                ));
            }
        }
    });
}

/// Everything one serve knows about its Tailscale leg: the mode it is in, the
/// collaborators a live mode change acts through, and the watcher generation that
/// makes a stale watcher's command harmless.
///
/// The serve loop is the ONLY writer of every cell in here. A watcher reads the
/// bound cell and writes nothing; the Host guard reads the literals cell and
/// writes nothing.
pub(crate) struct TailscaleLoop {
    /// The mode the serve is in right now, which is NOT necessarily the
    /// configured one: `--no-tailscale` forces `no`, and a live change moves it.
    mode: TailscaleMode,
    /// Set by `--no-tailscale`. Refuses every live mode that wants Tailscale for
    /// as long as this run lasts; the config value still saves for the next one.
    forced_no: bool,
    /// The primary listener's address, which is where the leg's PORT comes from.
    /// `None` when it could not be read, in which case there is nothing to hang a
    /// leg on.
    primary: Option<SocketAddr>,
    /// What is bound right now. One cell per serve, written only here, read by
    /// whichever watcher is current.
    bound: Arc<std::sync::Mutex<Option<SocketAddr>>>,
    /// Whether the Host guard admits Tailscale IP literals.
    host_literals: Arc<AtomicBool>,
    /// Whether a watcher is running, so a dying best-effort leg's warning stays
    /// truthful across a live mode change.
    watched: Arc<AtomicBool>,
    /// The sender the loop keeps for the WHOLE serve, so the command lane never
    /// closes under it and there is no "the watcher has ended" state to track.
    /// Each watcher gets a clone stamped with its own generation.
    leg_tx: tokio::sync::mpsc::Sender<(u64, LegCommand)>,
    /// Taken once by the serve path and handed to the loop.
    leg_rx: Option<tokio::sync::mpsc::Receiver<(u64, LegCommand)>>,
    /// The current generation. Bumped by every watcher STOP (a start stops the
    /// watcher it replaces) and every one-shot detection, so a command from a
    /// watcher the mode already left is dropped rather than re-binding a leg
    /// that was just let go.
    generation: u64,
    /// The current watcher's stop flag, per watcher rather than per serve: a mode
    /// change stops exactly the watcher it is replacing.
    watcher_stop: Option<Arc<AtomicBool>>,
    /// The Tailscale address probe. Injected so the loop's mode transitions are
    /// testable without a Tailscale binary.
    detect: Arc<dyn Fn() -> Result<IpAddr, TailscaleUnavailable> + Send + Sync>,
}

/// A one-shot detection the loop is waiting on, kept OUT of the mode arm so the
/// parent lane, leg deaths and watcher commands keep flowing while it runs.
struct PendingDetect {
    /// The generation this detection belongs to. A newer request bumps the
    /// counter, and the answer of an older one is discarded.
    generation: u64,
    mode: TailscaleMode,
    reply: tokio::sync::oneshot::Sender<TailscaleModeOutcome>,
    task: tokio::task::JoinHandle<Result<IpAddr, TailscaleUnavailable>>,
}

impl TailscaleLoop {
    pub(crate) fn new(
        mode: TailscaleMode,
        forced_no: bool,
        primary: Option<SocketAddr>,
        initial_leg: Option<SocketAddr>,
        control: &TailscaleModeControl,
        detect: Arc<dyn Fn() -> Result<IpAddr, TailscaleUnavailable> + Send + Sync>,
    ) -> Self {
        let (leg_tx, leg_rx) = tokio::sync::mpsc::channel(LEG_COMMAND_QUEUE);
        Self {
            mode,
            forced_no,
            primary,
            bound: Arc::new(std::sync::Mutex::new(initial_leg)),
            host_literals: control.host_literals(),
            watched: control.watched(),
            leg_tx,
            leg_rx: Some(leg_rx),
            generation: 0,
            watcher_stop: None,
            detect,
        }
    }

    /// The command lane the serve loop listens on. Taken once.
    pub(crate) fn take_leg_receiver(&mut self) -> tokio::sync::mpsc::Receiver<(u64, LegCommand)> {
        self.leg_rx.take().expect("the leg receiver is taken once")
    }

    /// Start the startup watcher when the mode is `auto`. It PARKS first: the
    /// startup bind answered the same question a moment ago.
    pub(crate) fn start_watcher_if_wanted(&mut self) {
        if self.mode.watches_interface() {
            self.start_watcher(false);
        }
    }

    fn bound_addr(&self) -> Option<SocketAddr> {
        self.bound.lock().ok().and_then(|slot| *slot)
    }

    /// End the running watcher, if any, and say so in the shared flag.
    ///
    /// The generation moves on unconditionally, because the stop flag alone does
    /// not stop a watcher already past its post-probe check: its command is still
    /// coming, and only a stale generation makes it harmless.
    fn stop_watcher(&mut self) {
        if let Some(stop) = self.watcher_stop.take() {
            stop.store(true, Ordering::SeqCst);
        }
        self.generation += 1;
        self.watched.store(false, Ordering::SeqCst);
    }

    /// Spawn a watcher over the EXISTING bound cell and command sender, stamped
    /// with the generation the stop above it minted. A dedicated std thread, never a runtime worker:
    /// the probe is a bounded but blocking call, and a wedged `tailscaled` (a
    /// suspend and resume, which is the exact scenario the watcher serves) must
    /// not be able to occupy a tokio worker.
    fn start_watcher(&mut self, probe_now: bool) -> bool {
        self.stop_watcher();
        let Some(primary) = self.primary else {
            dux_core::logger::warn(
                "[server] not starting the Tailscale interface watcher: the address of the \
                 primary listener could not be read, so there is no port to serve the Tailscale \
                 leg on. dux is serving on the addresses it bound at startup.",
            );
            return false;
        };
        let generation = self.generation;
        let stop = Arc::new(AtomicBool::new(false));
        let watcher_stop = Arc::clone(&stop);
        let bound = Arc::clone(&self.bound);
        let tx = self.leg_tx.clone();
        let detect = Arc::clone(&self.detect);
        let started = std::thread::Builder::new()
            .name("dux-tailscale-watch".to_string())
            .spawn(move || {
                watch_tailscale_leg(
                    primary,
                    WATCH_PERIOD,
                    probe_now,
                    &*detect,
                    &|| bound.lock().ok().and_then(|slot| *slot),
                    &|command| tx.blocking_send((generation, command)).is_ok(),
                    &|| watcher_stop.load(Ordering::SeqCst),
                );
            })
            .is_ok();
        if started {
            self.watcher_stop = Some(stop);
            self.watched.store(true, Ordering::SeqCst);
        } else {
            // A thread dux cannot start is not a reason to refuse to serve; it is
            // a reason to say the Tailscale leg is now static for this run.
            dux_core::logger::warn(
                "[server] could not start the Tailscale interface watcher. dux is serving on \
                 the addresses it bound at startup; restart dux to pick up a Tailscale address \
                 that appears later.",
            );
        }
        started
    }

    /// Serving is over: let the current watcher finish its park and exit rather
    /// than probing a server that has stopped.
    pub(crate) fn shutdown(&mut self) {
        self.stop_watcher();
    }

    /// The "what is bound" cell, so a test can read what the loop did without
    /// opening a socket to look.
    #[cfg(test)]
    pub(crate) fn bound_cell(&self) -> Arc<std::sync::Mutex<Option<SocketAddr>>> {
        Arc::clone(&self.bound)
    }

    /// A generation-stamping sender, so a test can play the part of a watcher.
    #[cfg(test)]
    pub(crate) fn leg_sender(&self) -> tokio::sync::mpsc::Sender<(u64, LegCommand)> {
        self.leg_tx.clone()
    }
}

/// The serve loop: hold the per-leg serve tasks, act on the Tailscale watcher's
/// commands and on live mode changes, and end only when the shutdown lane is
/// tripped.
///
/// Deliberately NOT a `while let Some(..) = join_next()` drain: with a watcher
/// running, an empty task set is a legitimate mid-life state (the required leg is
/// there, but a moment where every task has just been replaced is possible), and
/// exiting on set-empty would end a server nobody asked to stop. The exit
/// condition is the shutdown lane and nothing else.
async fn run_serve_loop(
    mut tasks: tokio::task::JoinSet<()>,
    shutdown: ServeShutdown,
    mut commands: tokio::sync::mpsc::Receiver<(u64, LegCommand)>,
    mut mode_requests: tokio::sync::mpsc::Receiver<crate::serve_legs::ModeRequest>,
    app: Router,
    console: Console,
    mut ts: TailscaleLoop,
) {
    let mut parent = shutdown.subscribe();
    // The address whose bind failed on the previous attempt, so a permanently
    // occupied Tailscale port is reported once instead of once every period. The
    // retry itself is deliberate (a port frees up, an interface finishes coming
    // up); saying the same sentence forever is not.
    let mut last_bind_failure: Option<SocketAddr> = None;
    let mut pending_detect: Option<PendingDetect> = None;
    let mut mode_lane_open = true;
    while !*parent.borrow_and_update() {
        tokio::select! {
            _ = parent.changed() => {}
            request = mode_requests.recv(), if mode_lane_open => {
                match request {
                    Some(request) => {
                        apply_mode_request(
                            request,
                            &mut ts,
                            &mut pending_detect,
                            &mut tasks,
                            &shutdown,
                            &app,
                            &console,
                            &mut last_bind_failure,
                        )
                        .await;
                    }
                    // Every control handle has been dropped, so nothing can ask
                    // for a mode change any more. The arm MUST be disabled: a
                    // closed receiver resolves instantly and forever, and an arm
                    // left enabled over one is a spin that starves the runtime
                    // this loop shares with every serve leg.
                    None => mode_lane_open = false,
                }
            }
            detected = async {
                (&mut pending_detect
                    .as_mut()
                    .expect("guarded by the arm's condition")
                    .task)
                    .await
            }, if pending_detect.is_some() => {
                let pending = pending_detect
                    .take()
                    .expect("guarded by the arm's condition");
                finish_detection(
                    pending,
                    detected.unwrap_or(Err(TailscaleUnavailable::CommandFailed)),
                    &mut ts,
                    &mut tasks,
                    &shutdown,
                    &app,
                    &console,
                    &mut last_bind_failure,
                )
                .await;
            }
            Some(joined) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(join_err) = joined {
                    // A task PANICKED and recorded nothing, so record it here and
                    // trip the lane: a panicking serve task is not a leg going
                    // quietly, it is a bug, and the server must not limp on
                    // half-dead. So the other listeners are shut down too.
                    dux_core::logger::error(&format!(
                        "[server] a serve task panicked: {join_err}. Shutting the other \
                         listeners down so the server does not limp on half-dead."
                    ));
                    shutdown.record_failure(anyhow::anyhow!(
                        "a serve task panicked: {join_err}"
                    ));
                }
                // A leg's task has ended. If it was the Tailscale leg dying on its
                // own (its accept loop failed mid-run, or it panicked), it forgot
                // itself from the registry but nothing cleared the watcher-facing
                // "what is bound" cell, and a watcher that still believes the leg
                // is bound plans Nothing forever: the leg would only come back
                // after a full interface flap. Reconcile here, where the serve loop
                // is the cell's ONE writer, rather than letting a dying task write
                // it from underneath.
                reconcile_bound_tailscale(&shutdown, &ts.bound, &mut last_bind_failure);
            }
            command = commands.recv() => {
                // The lane never closes while serving: the loop holds a sender
                // for its whole life, so a mode that runs no watcher simply has
                // nobody sending. A `None` therefore means the loop's own state
                // is gone, which cannot happen before this arm stops running.
                if let Some((generation, command)) = command {
                    // A watcher parked in a five-second probe when the mode
                    // changed comes back with a command for the mode dux already
                    // left. Its generation is stale, so it is dropped: acting on
                    // it would re-bind the leg the change just let go.
                    if generation == ts.generation {
                        apply_leg_command(
                            command,
                            &mut tasks,
                            &shutdown,
                            &app,
                            &console,
                            &ts.bound,
                            &mut last_bind_failure,
                        )
                        .await;
                    }
                }
            }
        }
    }
    // An in-flight detection has nobody left to report to; say so rather than
    // dropping its lane, which the caller would read as "not serving".
    if let Some(pending) = pending_detect.take() {
        pending.task.abort();
        let _ = pending.reply.send(TailscaleModeOutcome::NotServing);
    }
    ts.shutdown();
    // The lane is tripped and `trigger` has fanned out to every leg, so each task
    // is winding down. Reap them; the CALLER bounds how long it waits for this.
    while tasks.join_next().await.is_some() {}
}

/// Carry out one live `[server] tailscale` change and answer the caller.
///
/// Every request is answered exactly once: here for the steps that finish
/// immediately, and in [`finish_detection`] for a `yes`, whose bounded probe runs
/// as its own select arm so the parent lane and the legs keep flowing under it.
#[allow(clippy::too_many_arguments)]
async fn apply_mode_request(
    request: crate::serve_legs::ModeRequest,
    ts: &mut TailscaleLoop,
    pending_detect: &mut Option<PendingDetect>,
    tasks: &mut tokio::task::JoinSet<()>,
    shutdown: &ServeShutdown,
    app: &Router,
    console: &Console,
    last_bind_failure: &mut Option<SocketAddr>,
) {
    let crate::serve_legs::ModeRequest { mode, reply } = request;
    // A request that arrives while a probe is in flight replaces it. The older
    // caller is told so rather than being left holding a lane that never answers.
    if let Some(previous) = pending_detect.take() {
        previous.task.abort();
        let _ = previous.reply.send(TailscaleModeOutcome::Superseded);
    }

    let steps = plan_mode_change(ts.mode, mode, ts.bound_addr(), ts.forced_no);
    let mut detached = false;
    for step in steps {
        match step {
            ModeStep::Refuse => {
                let _ = reply.send(TailscaleModeOutcome::RefusedForcedNo);
                return;
            }
            ModeStep::StopWatcher => ts.stop_watcher(),
            ModeStep::SetHostLiterals(allowed) => ts.host_literals.store(allowed, Ordering::SeqCst),
            ModeStep::Unbind(addr) => {
                apply_leg_command(
                    LegCommand::Unbind(addr),
                    tasks,
                    shutdown,
                    app,
                    console,
                    &ts.bound,
                    last_bind_failure,
                )
                .await;
                detached = true;
            }
            ModeStep::StartWatcher { probe_now } => {
                ts.mode = mode;
                if !ts.start_watcher(probe_now) {
                    let _ = reply.send(TailscaleModeOutcome::NoPrimary);
                    return;
                }
            }
            ModeStep::DetectAndBind => {
                ts.mode = mode;
                if ts.primary.is_none() {
                    let _ = reply.send(TailscaleModeOutcome::NoPrimary);
                    return;
                }
                // A generation of its own, so a watcher this change replaced
                // cannot land a command against the answer of this probe.
                ts.generation += 1;
                let detect = Arc::clone(&ts.detect);
                *pending_detect = Some(PendingDetect {
                    generation: ts.generation,
                    mode,
                    reply,
                    // `spawn_blocking`, never the loop: the probe is a bounded
                    // but blocking subprocess call, and awaiting it inline would
                    // stop the legs and the parent lane for its whole window.
                    task: tokio::task::spawn_blocking(move || detect()),
                });
                return;
            }
        }
    }
    ts.mode = mode;
    let outcome = if detached {
        TailscaleModeOutcome::Detached
    } else {
        TailscaleModeOutcome::Applied {
            bound: ts.bound_addr(),
        }
    };
    let _ = reply.send(outcome);
}

/// Act on a one-shot detection's answer and resolve the request that asked for
/// it.
///
/// A FAILED probe deliberately leaves a leg that is already serving alone: `yes`
/// means "bind it once and keep it", and dropping a working listener because one
/// local daemon call did not answer is the wrong way to be wrong.
#[allow(clippy::too_many_arguments)]
async fn finish_detection(
    pending: PendingDetect,
    detected: Result<IpAddr, TailscaleUnavailable>,
    ts: &mut TailscaleLoop,
    tasks: &mut tokio::task::JoinSet<()>,
    shutdown: &ServeShutdown,
    app: &Router,
    console: &Console,
    last_bind_failure: &mut Option<SocketAddr>,
) {
    let PendingDetect {
        generation,
        mode,
        reply,
        task: _,
    } = pending;
    if generation != ts.generation {
        let _ = reply.send(TailscaleModeOutcome::Superseded);
        return;
    }
    let Some(primary) = ts.primary else {
        let _ = reply.send(TailscaleModeOutcome::NoPrimary);
        return;
    };
    let already = ts.bound_addr();
    if detected.is_err() {
        let _ = reply.send(match already {
            Some(addr) => TailscaleModeOutcome::Applied { bound: Some(addr) },
            None => TailscaleModeOutcome::NothingDetected,
        });
        return;
    }
    let desired = desired_leg(primary, detected);
    // The idempotence guard: an address that is already bound plans `Nothing`,
    // so choosing the mode dux is already in never re-binds dux's own port and
    // reports an EADDRINUSE that says nothing is wrong.
    for command in match plan_leg_step(already, desired) {
        LegStep::Nothing => Vec::new(),
        LegStep::Bind(addr) => vec![LegCommand::Bind(addr)],
        LegStep::Unbind(addr) => vec![LegCommand::Unbind(addr)],
        LegStep::Rebind { old, new } => vec![LegCommand::Unbind(old), LegCommand::Bind(new)],
    } {
        apply_leg_command(
            command,
            tasks,
            shutdown,
            app,
            console,
            &ts.bound,
            last_bind_failure,
        )
        .await;
    }
    let bound = ts.bound_addr();
    let _ = reply.send(match (desired, bound) {
        // A leg was wanted and none is up: the address is there but its listener
        // would not bind.
        (Some(_), None) => TailscaleModeOutcome::BindFailed,
        _ => {
            let _ = mode;
            TailscaleModeOutcome::Applied { bound }
        }
    });
}

/// Reconcile the watcher-facing "what is bound" cell against the leg registry,
/// which is the one authority on what dux is actually serving.
///
/// The cell exists so the watcher compares against reality rather than against
/// what it last asked for. A best-effort leg can leave that reality WITHOUT the
/// serve loop having asked: its accept loop dies, `record_best_effort_failure`
/// forgets it from the registry, and its task ends. Then the cell would still name
/// the address, the watcher's plan would be `Nothing` on every period, and the leg
/// would stay down until the interface itself flapped. So whenever a leg's task
/// ends, an address the cell names but the registry does not is cleared.
///
/// The bind-failure streak is cleared with it: a bind attempt after the leg went
/// away is a NEW streak, and its failure deserves the warning that the first one
/// got rather than a debug line about a streak that ended.
///
/// This keeps the cell single-writer (the serve loop). Letting the dying task
/// clear it would mean two writers racing over one `Option`, and a Rebind's
/// unbind-then-bind pair could have the loser blank an address that had just been
/// bound.
fn reconcile_bound_tailscale(
    shutdown: &ServeShutdown,
    bound_tailscale: &Arc<std::sync::Mutex<Option<SocketAddr>>>,
    last_bind_failure: &mut Option<SocketAddr>,
) {
    let Ok(mut slot) = bound_tailscale.lock() else {
        return;
    };
    if let Some(addr) = *slot
        && !shutdown.has_leg(addr)
    {
        *slot = None;
        *last_bind_failure = None;
        dux_core::logger::debug(&format!(
            "[server] the Tailscale leg on {addr} is no longer serving; the watcher will \
             bind it again on its next period while the interface is there."
        ));
    }
}

/// Act on one watcher command: bind and start serving the Tailscale leg, or stop
/// it. Records what is bound so the watcher's next period compares against
/// reality (which is what makes a failed bind retry rather than vanish).
async fn apply_leg_command(
    command: LegCommand,
    tasks: &mut tokio::task::JoinSet<()>,
    shutdown: &ServeShutdown,
    app: &Router,
    console: &Console,
    bound_tailscale: &Arc<std::sync::Mutex<Option<SocketAddr>>>,
    last_bind_failure: &mut Option<SocketAddr>,
) {
    match command {
        LegCommand::Bind(addr) => match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                *last_bind_failure = None;
                spawn_leg(
                    tasks,
                    app.clone(),
                    listener,
                    addr,
                    false,
                    shutdown,
                    console.clone(),
                );
                if let Ok(mut slot) = bound_tailscale.lock() {
                    *slot = Some(addr);
                }
                let message = format!(
                    "Tailscale interface is back: dux is now also serving on http://{addr}. \
                     Nothing else changed; your other address is untouched."
                );
                dux_core::logger::info(&format!("[server] {message}"));
                console.leg_changed(&message);
            }
            Err(err) => {
                // Best-effort: say so and carry on. The watcher compares against
                // what is BOUND, so it asks again next period. Say it ONCE per
                // streak, though: a port somebody else holds permanently would
                // otherwise repeat the same sentence for as long as dux runs.
                let warning = tailscale_bind_warning(addr, &err);
                if *last_bind_failure == Some(addr) {
                    dux_core::logger::debug(&format!("[server] still {warning}"));
                } else {
                    dux_core::logger::warn(&format!("[server] {warning}"));
                    console.bind_degraded(&warning);
                }
                *last_bind_failure = Some(addr);
                if let Ok(mut slot) = bound_tailscale.lock() {
                    *slot = None;
                }
            }
        },
        LegCommand::Unbind(addr) => {
            let stopped = shutdown.stop_leg(addr);
            if let Ok(mut slot) = bound_tailscale.lock() {
                *slot = None;
            }
            // The interface went away, so the once-per-streak suppression ends
            // here: the next bind attempt is a fresh situation, and a port that is
            // still busy after a flap deserves to be said out loud again rather
            // than being silenced by a streak that predates the flap.
            *last_bind_failure = None;
            if stopped {
                let message = format!(
                    "Tailscale interface went away: dux stopped serving on {addr} and is still \
                     serving on its other address(es). Browsers that were on the tailnet \
                     reconnect by themselves when it comes back."
                );
                dux_core::logger::info(&format!("[server] {message}"));
                console.leg_changed(&message);
            }
        }
    }
}

/// The hooks only the BACKGROUND serve wants, because it is the only serve with a
/// terminal UI beside it: somewhere for `build_app` to leave this serve's
/// ownership publisher, and a counter for its connection registry to keep the live
/// browser-tab count in.
///
/// Bundled rather than passed side by side. They travel together through the same
/// three functions, every other serve path passes the default, and two adjacent
/// slots is the shape where a third one gets threaded into two callers out of
/// three.
#[derive(Default)]
pub(crate) struct BackgroundHooks {
    /// See [`RouterParams::with_ownership_publisher`].
    pub(crate) ownership_publisher:
        Option<Arc<std::sync::OnceLock<crate::ownership_publish::OwnershipPublisher>>>,
    /// See [`RouterParams::with_connections_gauge`].
    pub(crate) connections_gauge: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

/// Build the router parameters every serve path derives from the same
/// `[server]` fields.
///
/// Existed as three near-identical blocks of `.with_*` calls before, which is
/// exactly the shape where a new limit gets threaded into two of the three and
/// nobody notices until the third one behaves differently.
fn router_params(
    config: &dux_core::config::Config,
    console: Console,
    access_log: bool,
    bound_ips: Vec<std::net::IpAddr>,
    hooks: BackgroundHooks,
) -> RouterParams {
    let BackgroundHooks {
        ownership_publisher,
        connections_gauge,
    } = hooks;
    let server = &config.server;
    let params = RouterParams::plain_http()
        .with_console(console, access_log)
        .with_max_websocket_connections(
            server.max_websocket_events_connections,
            server.max_websocket_agent_connections,
            server.max_websocket_terminal_connections,
            server.max_websocket_tab_connections,
            server.max_websocket_tabs_per_agent,
        )
        .with_search_index_max_files(server.search_index_max_files)
        .with_tree_list_max_concurrency(server.tree_list_max_concurrency)
        .with_release_notes_max_concurrency(server.release_notes_max_concurrency)
        .with_file_drop_limits(server.file_drop_max_bytes, server.file_drop_max_concurrency)
        .with_host_allowlist(
            bound_ips,
            server.allowed_hosts.clone(),
            server.tailscale_mode().wants_tailscale(),
        );
    let params = match ownership_publisher {
        Some(slot) => params.with_ownership_publisher(slot),
        None => params,
    };
    match connections_gauge {
        Some(gauge) => params.with_connections_gauge(gauge),
        None => params,
    }
}

/// Whether a serve owns the process's stop signals.
pub(crate) enum SignalPolicy {
    /// Install the tokio SIGINT/SIGTERM handler and trip this flag when one
    /// arrives. For a serve that is the only thing running: the flip, whose
    /// status screen has taken the terminal over.
    Adopt(Arc<AtomicBool>),
    /// Install NOTHING. Another surface's handlers already own the process, and
    /// two sets of handlers for one signal is a race over who tears down what.
    /// The background server runs behind a live terminal UI whose own handlers
    /// drive its quit, and the quit stops the serve on its way out.
    Inherited,
}

/// One serving lifetime: the tokio runtime it all runs on, the shutdown lanes,
/// the Tailscale watcher's stop flag, and the supervisor task holding the legs.
///
/// The runtime IS the reaper. Everything `build_app` spawns (the changed-files
/// poller, the config-reload forwarder, the spine-change forwarder, the
/// first-load resolver) plus every serve leg lives on this runtime and on no
/// other, so dropping it ends all of them at once. That is what makes a
/// stop/start cycle safe: the second serve builds a fresh app with fresh
/// registries rather than adding a second poller beside the first. The cost,
/// accepted and stated: connection and PTY-ownership state resets across a
/// cycle, and browsers reconnect in place (same process identity, so no reload).
pub(crate) struct ServeCore {
    /// Torn down explicitly by [`Self::finish`], never by an implicit drop of this
    /// struct if it can be helped: `Runtime::drop` blocks until every
    /// `spawn_blocking` task returns and cannot abort them, so a parked PTY
    /// forwarder would hang it forever. `finish` uses `shutdown_timeout`, which
    /// detaches stragglers. An implicit drop is still reachable on a start-time
    /// error path, where no forwarder is parked on this runtime yet.
    runtime: tokio::runtime::Runtime,
    shutdown: ServeShutdown,
    /// The live Tailscale-mode handle this serve built, so the caller can hand it
    /// to a surface that changes the mode from outside the router (the terminal
    /// UI's companion in background mode).
    tailscale_mode: TailscaleModeControl,
    /// Taken by [`Self::wind_down`], which joins it. `None` afterwards.
    supervisor: Option<tokio::task::JoinHandle<()>>,
    /// Which branch of [`SignalPolicy`] this serve actually took. Recorded so
    /// "the background server installs no signal handlers" is a checkable fact
    /// about the code path rather than a claim in a comment.
    installed_signal_handlers: bool,
}

impl ServeCore {
    /// Adopt pre-bound std listeners and start serving.
    ///
    /// STD-BIND-THEN-ADOPT is the whole point of taking bound listeners rather
    /// than addresses: the caller binds while its own surface is still up and
    /// fully functional, so a port collision is a message on a status line and
    /// nothing has been torn down. By the time this runs, the addresses are
    /// already ours and there is no rebind race.
    ///
    /// `handle` is consumed: the router keeps its own clones, so holding one here
    /// would keep the request channel alive past the point where the serve is
    /// meant to be over.
    pub(crate) fn start(
        handle: engine_actor::EngineHandle,
        listeners: Vec<std::net::TcpListener>,
        config: &dux_core::config::Config,
        console: Console,
        access_log: bool,
        signals: SignalPolicy,
        // Passed straight through to `build_app`. Default for every serve but the
        // background one, which is the only one with a terminal UI beside it.
        hooks: BackgroundHooks,
    ) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        // Read everything the router and the watcher need from the std listeners
        // BEFORE the conversion loop below moves them into the tokio set.
        let bound_ips: Vec<std::net::IpAddr> = listeners
            .iter()
            .filter_map(|l| l.local_addr().ok())
            .map(|a| a.ip())
            .collect();
        // Both of these serves are structurally LOCAL MODE: loopback plus, when
        // wanted, the Tailscale leg. So the primary is the loopback listener and
        // the watched port is whatever the caller's pre-flight bound it on (which
        // may be ephemeral).
        let tailscale = config.server.tailscale_mode();
        let primary = listeners
            .iter()
            .filter_map(|l| l.local_addr().ok())
            .find(|a| a.ip().is_loopback());
        let tailscale_leg = listeners
            .iter()
            .filter_map(|l| l.local_addr().ok())
            .find(|a| !a.ip().is_loopback());

        // The std listeners travel here already bound, so there is no rebind race;
        // tokio needs them non-blocking. Adoption failures are rare (the bind
        // already succeeded in the caller's pre-flight), but log the failing
        // address before propagating so a serve that cannot start leaves a
        // forensic record in dux.log, not just a status line.
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
                        "[server] could not adopt the pre-bound listener on {addr} \
                         (set_nonblocking failed): {err}"
                    ));
                    return Err(err.into());
                }
                match tokio::net::TcpListener::from_std(listener) {
                    Ok(l) => out.push(l),
                    Err(err) => {
                        dux_core::logger::error(&format!(
                            "[server] could not adopt the pre-bound listener on {addr} \
                             (tokio from_std failed): {err}"
                        ));
                        return Err(err.into());
                    }
                }
            }
            out
        };

        // The live-mode collaborators, created BEFORE the router so the Host
        // guard reads the mode from the same cell the serve loop writes. Neither
        // in-app mode is ever a forced-no run: `--no-tailscale` is a `dux server`
        // flag, so both can change the mode live.
        let (mode_control, mode_requests) = TailscaleModeControl::new(
            runtime.handle().clone(),
            Arc::new(AtomicBool::new(tailscale.watches_interface())),
            Arc::new(AtomicBool::new(tailscale.wants_tailscale())),
        );
        handle.set_tailscale_mode_control(mode_control.clone());

        // The shared shutdown primitive — the SAME [`ServeShutdown`] the CLI serve
        // path uses. Its watch is the graceful-shutdown lane every serve task and
        // the sweep await; a dying listener flips it via `record_failure`.
        let shutdown = ServeShutdown::new(mode_control.watched());

        // Build ONE app, shared across listeners (the router is a cheap
        // `Arc`-backed service). `build_app` constructs the `ChangesService`,
        // which spawns its supervised poller via `tokio::spawn` -- that needs an
        // entered runtime.
        let app = {
            let _guard = runtime.enter();
            server::build_app(
                handle.clone(),
                axum::Router::new(),
                router_params(config, console.clone(), access_log, bound_ips, hooks)
                    .with_live_tailscale_host_literals(mode_control.host_literals())
                    .with_tailscale_mode_control(mode_control.clone(), false),
            )
        };

        // On `auto`, watch the Tailscale interface for the rest of the serve.
        let mut tailscale_loop = {
            let _guard = runtime.enter();
            let mut tailscale_loop = TailscaleLoop::new(
                tailscale,
                false,
                // No loopback listener address means no port to hang the leg on,
                // so this deliberately starts no watcher rather than guessing one.
                primary,
                tailscale_leg,
                &mode_control,
                Arc::new(dux_core::tailscale::detect_ip),
            );
            tailscale_loop.start_watcher_if_wanted();
            tailscale_loop
        };
        let leg_commands = tailscale_loop.take_leg_receiver();

        // One serve leg per listener plus the loop that acts on the watcher, all
        // as ONE supervisor task, so teardown is a single bounded join and a leg
        // the watcher added later winds down through the same trigger.
        let supervisor = {
            let shutdown = shutdown.clone();
            let app = app.clone();
            let console = console.clone();
            let mut legs = tokio::task::JoinSet::new();
            let guard = runtime.enter();
            for tokio_listener in tokio_listeners {
                // Loopback is the REQUIRED leg (there is nothing to serve
                // without it); the Tailscale leg is best-effort, exactly as in
                // the CLI path. A listener whose own address cannot be read is
                // served BEST-EFFORT: it cannot be identified as the loopback
                // leg, and treating an unknown address as the one whose death
                // ends the whole server is the wrong way to be wrong. Its
                // placeholder address is a registry key and a log label only,
                // never a bind target.
                let (addr, required) = match tokio_listener.local_addr() {
                    Ok(addr) => (addr, addr.ip().is_loopback()),
                    Err(err) => {
                        dux_core::logger::warn(&format!(
                            "[server] serving a pre-bound listener whose address could not be \
                             read ({err}); it is treated as best-effort, so its failure alone \
                             will not stop the server. The web UI is still reachable on the \
                             addresses dux reported."
                        ));
                        (SocketAddr::from(([127, 0, 0, 1], 0)), false)
                    }
                };
                spawn_leg(
                    &mut legs,
                    app.clone(),
                    tokio_listener,
                    addr,
                    required,
                    &shutdown,
                    console.clone(),
                );
            }
            drop(guard);
            runtime.spawn(run_serve_loop(
                legs,
                shutdown,
                leg_commands,
                mode_requests,
                app,
                console,
                tailscale_loop,
            ))
        };
        // The router holds its own cloned handle(s); drop ours so only the serve
        // tasks keep the request side alive.
        drop(handle);

        let installed_signal_handlers = match signals {
            SignalPolicy::Adopt(flag) => {
                runtime.spawn(async move {
                    shutdown_signal().await;
                    flag.store(true, Ordering::SeqCst);
                });
                true
            }
            // Nothing installed on purpose. See `SignalPolicy::Inherited`.
            SignalPolicy::Inherited => false,
        };

        Ok(Self {
            runtime,
            shutdown,
            tailscale_mode: mode_control,
            supervisor: Some(supervisor),
            installed_signal_handlers,
        })
    }

    /// Whether a required leg's accept loop died, so the caller can stop serving
    /// rather than limp on.
    pub(crate) fn is_failed(&self) -> bool {
        self.shutdown.is_failed()
    }

    /// Whether this serve installed the process's SIGINT/SIGTERM handlers.
    pub(crate) fn installed_signal_handlers(&self) -> bool {
        self.installed_signal_handlers
    }

    /// This serve's live Tailscale-mode handle, for a surface that changes the
    /// mode from outside the router.
    pub(crate) fn tailscale_mode(&self) -> TailscaleModeControl {
        self.tailscale_mode.clone()
    }

    /// Stop the listeners and reap the supervisor, bounded, WITHOUT tearing the
    /// runtime down yet.
    ///
    /// `shutdown_flag` is tripped FIRST, before anything waits. The PTY
    /// forwarders are parked inside a blocking `recv_timeout` on channels the
    /// engine still owns, so nothing disconnects them on its own; without the flag
    /// the runtime teardown would block on tasks it cannot abort, and the caller
    /// (a terminal UI, mid-keystroke) would freeze.
    ///
    /// Separate from [`Self::finish`] because the flip has work to do in between:
    /// its quit path SIGTERMs the agents and waits out the grace window, and
    /// `shutdown_signal`'s second-signal force-quit watcher is a task on this
    /// still-alive runtime. Tearing the runtime down first would kill that
    /// watcher and take the operator's escape hatch with it.
    pub(crate) fn wind_down(&mut self, shutdown_flag: &Arc<AtomicBool>) {
        shutdown_flag.store(true, Ordering::SeqCst);

        // Trigger graceful axum shutdown and wait (bounded) for the supervisor to
        // reap every leg. `trigger` fans out over the leg registry, so a Tailscale
        // leg the watcher added mid-serve winds down here too rather than carrying
        // its listener into the surface that resumes. The bound keeps a wedged
        // client connection on any listener from hanging the caller.
        self.shutdown.trigger();
        if let Some(supervisor) = self.supervisor.take() {
            self.runtime.block_on(async {
                let _ = tokio::time::timeout(SERVER_JOIN_TIMEOUT, supervisor).await;
            });
        }
    }

    /// Tear the runtime down and report the first listener failure, if any.
    ///
    /// Bounded rather than an implicit `drop(runtime)`, which would block forever
    /// on any parked `spawn_blocking` task (drop cannot abort them);
    /// `shutdown_timeout` detaches stragglers instead. Dropping the runtime is
    /// also what reaps everything `build_app` spawned, which is what makes the
    /// next serve's registries genuinely fresh.
    pub(crate) fn finish(self) -> Option<anyhow::Error> {
        self.runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
        self.shutdown.take_error()
    }

    /// Wind down and tear down in one step, for a caller with nothing to do in
    /// between.
    pub(crate) fn stop(mut self, shutdown_flag: &Arc<AtomicBool>) -> Option<anyhow::Error> {
        self.wind_down(shutdown_flag);
        self.finish()
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
///
/// `on_shutdown_status` is called with a human-readable teardown message (e.g.
/// "Stopping 2 agents...") once QuitProcess teardown starts. This crate has no
/// terminal of its own, so it hands the message to the caller instead of
/// printing it directly; the binary's implementation feeds it to the dux-tui
/// status screen, which renders it on its own themed line rather than raw text
/// landing wherever the cursor happens to sit.
pub fn serve_with_engine(
    mut engine: Engine,
    listeners: Vec<std::net::TcpListener>,
    activity: dux_core::activity::ActivityRing,
    mut on_tick: impl FnMut() -> ServerTick,
    mut on_shutdown_status: impl FnMut(&str),
) -> Result<(Engine, ServerExit)> {
    warn_if_ui_not_built();
    // The flip owns the terminal with its themed status screen, so this console
    // writes NOTHING to stdout — but it captures every lifecycle event into the
    // shared ring that drives the status screen's Activity panel.
    let console = Console::capture(activity);
    let (handle, ends) = engine_actor::build_actor_channels(&engine);
    engine_actor::spawn_global_workers(&mut engine);

    // Grab the teardown flag before the handle moves into the router. `ServeCore`
    // trips it the instant serving ends (before axum graceful shutdown) so any
    // PTY forwarders parked on their blocking `recv_timeout` exit within one poll
    // window — even on ReturnToTui, where the engine and its PtyClient senders
    // stay alive and the forwarders' channels would otherwise never disconnect.
    let shutdown_flag = handle.shutdown_flag();

    // Set by the signal task; polled by the control closure so a SIGINT/SIGTERM
    // received while serving breaks the engine loop too (not just axum). Distinct
    // from the serve's failure flag because a signal means QuitProcess, a failure
    // means ReturnToTui-with-error.
    let signal_quit = Arc::new(AtomicBool::new(false));

    // The flip has taken the terminal over with its own status screen, so it OWNS
    // the process's stop signals for as long as it serves.
    let mut core = ServeCore::start(
        handle,
        listeners,
        &engine.config,
        console,
        // The capture console keeps the access log OFF (it is never wanted in the
        // panel, and access() never reaches emit() to be captured anyway) while
        // the WS handlers feed lifecycle events into the ring.
        false,
        SignalPolicy::Adopt(Arc::clone(&signal_quit)),
        // The flip has no terminal UI beside it (it took the terminal over), so
        // there is no second surface to announce ownership changes for, and nowhere
        // to show a connection count either.
        BackgroundHooks::default(),
    )?;

    // Run the engine loop on the CURRENT thread. The control closure decides the
    // exit reason: a serve failure or a tripped signal flag wins (both exit the
    // loop), otherwise the caller's tick result maps straight through.
    let mut exit = ServerExit::ReturnToTui;
    let mut engine = engine_actor::run_engine_loop(
        engine,
        ends,
        // The flip shares this terminal with a themed status screen, so shutdown
        // progress goes to dux.log and the caller's status callback, never to
        // stderr where it would land wherever the cursor happens to sit.
        engine_actor::ShutdownEcho::Silent,
        || {
            if core.is_failed() {
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
        },
    );

    // Stop the listeners and reap the serve, bounded. The runtime stays alive
    // past this point on purpose; see the quit block below.
    core.wind_down(&shutdown_flag);

    if matches!(exit, ServerExit::QuitProcess) {
        // Quit teardown: SIGTERM the children so CLIs can save state for a later
        // resume, mark agent sessions Detached. We own the engine here, so we
        // call `shutdown_ptys` directly (the dedicated-thread path routes the
        // equivalent through the `Shutdown` request). The grace window is the
        // configured `[server].shutdown_timeout_seconds` (web mode, even though
        // this was flipped from the TUI); `shutdown_ptys` logs to dux.log and we
        // also echo through the caller's status callback.
        //
        // Crucially this runs BEFORE the runtime teardown below: the wait can now
        // last up to the configured grace, and `shutdown_signal`'s second-signal
        // watcher is a task on this still-alive runtime, so a second
        // Ctrl-C/SIGTERM during the wait force-exits (130) instead of trapping the
        // operator behind a child that ignores SIGTERM. Tearing the runtime down
        // first would kill that watcher and remove the escape hatch.
        let agents = engine.providers.len();
        let terminals = engine.companion_terminals.len();
        if agents + terminals > 0 {
            let grace =
                dux_core::config::shutdown_grace(engine.config.server.shutdown_timeout_seconds);
            on_shutdown_status(&dux_core::engine::format_shutdown_start(
                agents, terminals, grace,
            ));
            let report = engine.shutdown_ptys(grace);
            on_shutdown_status(&dux_core::engine::format_shutdown_result(&report));
        }
    }

    let serve_error = core.finish();

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

    // If a listener's accept loop died, surface the captured error rather
    // than reporting a clean exit. The engine has already been wound down above,
    // so the caller drops it; the TUI shows the failure instead of resuming onto
    // a server that silently stopped serving.
    if let Some(err) = serve_error {
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
        Reachability, bind_plan_addrs, plain_http_banner, reachability, safety_note,
        tailscale_bind_warning,
    };
    use dux_core::config::{PlanAddr, TailscaleMode};
    use dux_core::engine::Command;

    #[test]
    fn reconciling_leaves_a_cell_that_still_names_a_live_leg_alone() {
        // The other half of the reconcile: a leg that is still serving must not be
        // blanked, or every reaped sibling task would have the watcher rebind a
        // healthy listener.
        let ts: std::net::SocketAddr = "100.64.0.5:8080".parse().unwrap();
        let shutdown = crate::serve_legs::ServeShutdown::for_watched(true);
        let _leg = shutdown.register_leg(ts);
        let cell = std::sync::Arc::new(std::sync::Mutex::new(Some(ts)));
        let mut streak = Some(ts);
        super::reconcile_bound_tailscale(&shutdown, &cell, &mut streak);
        assert_eq!(*cell.lock().unwrap(), Some(ts), "the leg is still serving");
        assert_eq!(streak, Some(ts), "and its failure streak is untouched");
    }

    #[tokio::test]
    async fn an_unbind_ends_the_bind_failure_streak_so_a_flap_warns_again() {
        // The once-per-streak suppression exists for a port somebody else holds
        // forever. An interface that went away and came back is a new situation,
        // and its bind failure must be said out loud rather than swallowed by a
        // streak that predates the flap.
        let ts: std::net::SocketAddr = "100.64.0.5:8080".parse().unwrap();
        let shutdown = crate::serve_legs::ServeShutdown::for_watched(true);
        let _leg = shutdown.register_leg(ts);
        let console = crate::console::Console::capture(dux_core::activity::ActivityRing::new());
        let cell = std::sync::Arc::new(std::sync::Mutex::new(Some(ts)));
        let mut streak = Some(ts);
        let mut tasks = tokio::task::JoinSet::new();

        super::apply_leg_command(
            crate::serve_legs::LegCommand::Unbind(ts),
            &mut tasks,
            &shutdown,
            &axum::Router::new(),
            &console,
            &cell,
            &mut streak,
        )
        .await;

        assert_eq!(*cell.lock().unwrap(), None, "nothing is bound there now");
        assert_eq!(
            streak, None,
            "the streak ends with the interface, so the next failure warns again"
        );
    }

    #[tokio::test]
    async fn a_best_effort_leg_that_died_on_its_own_is_bound_again_on_the_next_period() {
        // The uncovered twin of serve_legs' `a_failed_bind_is_retried_on_the_next_period`.
        // There the BIND failed; here the bind succeeded and the accept loop died
        // later, which is the routine case (the laptop suspended, tailscaled
        // stopped). The watcher compares against the "what is bound" cell, so a
        // death that leaves the cell naming the dead address makes every period
        // plan Nothing and the leg stays down until the interface itself flaps.
        let ts: std::net::SocketAddr = "100.64.0.5:8080".parse().unwrap();
        let shutdown = crate::serve_legs::ServeShutdown::for_watched(true);
        let leg_lane = shutdown.register_leg(ts);
        let console = crate::console::Console::capture(dux_core::activity::ActivityRing::new());

        // The leg's accept loop dies mid-run: exactly what `spawn_leg`'s
        // best-effort arm does, without the flakiness of forcing a real axum
        // accept loop to error.
        let mut legs = tokio::task::JoinSet::new();
        {
            let shutdown = shutdown.clone();
            legs.spawn(async move {
                shutdown.record_best_effort_failure(
                    ts,
                    &anyhow::anyhow!("the interface went away mid-serve"),
                );
            });
        }

        let (control, mode_rx) = crate::serve_legs::TailscaleModeControl::new(
            tokio::runtime::Handle::current(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        );
        let mut tailscale_loop = super::TailscaleLoop::new(
            TailscaleMode::Auto,
            false,
            Some("127.0.0.1:8080".parse().unwrap()),
            Some(ts),
            &control,
            std::sync::Arc::new(|| Err(dux_core::tailscale::TailscaleUnavailable::NoAddress)),
        );
        let commands_rx = tailscale_loop.take_leg_receiver();
        let bound_tailscale = tailscale_loop.bound_cell();
        let loop_task = tokio::spawn(super::run_serve_loop(
            legs,
            shutdown.clone(),
            commands_rx,
            mode_rx,
            axum::Router::new(),
            console,
            tailscale_loop,
        ));

        let cleared = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if bound_tailscale
                    .lock()
                    .expect("the cell is not poisoned")
                    .is_none()
                {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(
            cleared.is_ok(),
            "a leg that died must stop looking bound, or the watcher never asks for it again"
        );

        // Which is the whole point: the very next watch period, with the interface
        // still there, plans a fresh Bind rather than Nothing.
        let bound_now = *bound_tailscale.lock().expect("the cell is not poisoned");
        assert_eq!(
            crate::serve_legs::plan_leg_step(bound_now, Some(ts)),
            crate::serve_legs::LegStep::Bind(ts),
            "the next period must plan a re-bind"
        );

        drop(leg_lane);
        shutdown.trigger();
        tokio::time::timeout(std::time::Duration::from_secs(2), loop_task)
            .await
            .expect("a tripped lane must end the serve loop")
            .expect("the serve loop task joins");
    }

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
        // on Linux), held on an ephemeral port for the whole test so the leg is
        // genuinely busy. The bind-failure path doesn't care that it's not a real
        // Tailscale address, only that the entry is best-effort.
        let held = std::net::TcpListener::bind("127.0.0.2:0").expect("hold a best-effort addr");
        let held_addr = held.local_addr().expect("held addr");

        // The required leg asks for port 0 and lets the KERNEL pick a free port
        // at bind time. Probe-binding, reading the port back and dropping the
        // listener would hand the port to the whole machine for the length of
        // the gap and race anything else wanting an ephemeral port
        // (`dead_base_url()` in `crates/dux-core/tests/release_notes_fetch.rs`
        // avoids the same race). Port 0 closes the window entirely: there is no
        // moment where the port is free and unclaimed.
        let required_addr: std::net::SocketAddr =
            "127.0.0.1:0".parse().expect("a literal loopback addr");

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
        // `BoundListener::addr` echoes what was ASKED for, so with port 0 the
        // assertion above cannot tell a real listener from a recorded intention.
        // The listener's own address is the proof that a port was actually taken.
        let listening_on = bound[0].listener.local_addr().expect("a bound listener");
        assert_eq!(listening_on.ip(), required_addr.ip());
        assert_ne!(
            listening_on.port(),
            0,
            "the kernel assigned a real port to the required leg"
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
    fn safety_note_loopback_only_is_none_when_tailscale_is_off() {
        let addrs = vec![plan_addr("127.0.0.1:8080", true)];
        assert_eq!(safety_note(&addrs, TailscaleMode::No), None);
        // `yes` looked once and found nothing, so this run stays loopback-only and
        // there is genuinely nothing to warn about either.
        assert_eq!(safety_note(&addrs, TailscaleMode::Yes), None);
    }

    #[test]
    fn safety_note_loopback_only_on_auto_still_says_the_tailnet_can_arrive() {
        // The note is printed ONCE and the leg comes and goes behind it. A serve
        // that is watching the interface will be reachable on the tailnet the
        // moment the laptop reconnects, and saying nothing is the wrong half of
        // the truth.
        let addrs = vec![plan_addr("127.0.0.1:8080", true)];
        let note = safety_note(&addrs, TailscaleMode::Auto)
            .expect("a watching serve must still warn about the tailnet");
        assert!(note.contains("tailnet"), "must mention tailnet: {note}");
        assert!(
            note.contains("when the interface appears"),
            "must say the leg can arrive later: {note}"
        );
    }

    #[test]
    fn safety_note_loopback_plus_tailscale_mentions_tailnet() {
        let addrs = vec![
            plan_addr("127.0.0.1:8080", true),
            plan_addr("100.64.0.5:8080", false),
        ];
        let note =
            safety_note(&addrs, TailscaleMode::Yes).expect("must have a note for tailscale leg");
        assert!(note.contains("tailnet"), "must mention tailnet: {note}");
        assert!(
            note.contains("connected"),
            "must scope it to being connected to the tailnet: {note}"
        );
        assert!(
            !note.contains("NO login"),
            "tailscale note must NOT say NO login: {note}"
        );
    }

    #[test]
    fn safety_note_wildcard_primary_mentions_no_login() {
        let addrs = vec![plan_addr("0.0.0.0:8080", true)];
        let note = safety_note(&addrs, TailscaleMode::Auto).expect("must warn for 0.0.0.0");
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
        let note = safety_note(&addrs, TailscaleMode::Auto).expect("must warn for LAN primary");
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
        let banner = plain_http_banner("0.1.0", &legs, &[], None, None);
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
        let banner = plain_http_banner("0.1.0", &legs, &warnings, None, None);
        assert_eq!(banner.warnings, warnings);
    }

    #[test]
    fn plain_http_banner_warns_when_the_web_ui_was_not_built_in() {
        // A binary built with DUX_DISABLE_UI_BUILD serves a notice page instead of
        // the app. The operator who launched the server may never open a browser,
        // so the banner has to say it, and say it FIRST.
        let legs = vec![(addr("127.0.0.1:8080"), true)];
        let bind_warnings = vec!["Tailscale leg busy".to_string()];
        let banner = plain_http_banner(
            "0.1.0",
            &legs,
            &bind_warnings,
            None,
            Some(crate::web_assets::UI_NOT_BUILT_WARNING),
        );
        assert_eq!(banner.warnings.len(), 2, "both warnings must survive");
        assert_eq!(banner.warnings[0], crate::web_assets::UI_NOT_BUILT_WARNING);
        assert_eq!(banner.warnings[1], bind_warnings[0]);
        assert!(
            banner.warnings[0].contains("DUX_DISABLE_UI_BUILD"),
            "the warning must name the variable that caused it: {}",
            banner.warnings[0]
        );
    }

    #[test]
    fn plain_http_banner_warns_when_an_existing_dist_was_reused() {
        // The invisible case, and the reason the banner takes a message rather
        // than a bool. This binary serves a REAL single-page app with real hashed
        // assets, built at some unknown earlier time, so nothing about using it
        // reveals the problem. The banner is one of the only two places it is
        // said (dux.log is the other).
        let legs = vec![(addr("127.0.0.1:8080"), true)];
        let banner = plain_http_banner(
            "0.1.0",
            &legs,
            &[],
            None,
            crate::web_assets::ui_build_warning(crate::web_assets::UiBuildState::StaleReuse),
        );
        assert_eq!(banner.warnings.len(), 1, "the reuse must produce a row");
        assert_eq!(banner.warnings[0], crate::web_assets::UI_STALE_WARNING);
        assert!(
            !banner.warnings[0].contains("NO web UI"),
            "this binary HAS a web UI; the row must not say otherwise: {}",
            banner.warnings[0]
        );
    }

    #[test]
    fn plain_http_banner_omits_the_ui_warning_for_a_normal_build() {
        let legs = vec![(addr("127.0.0.1:8080"), true)];
        let banner = plain_http_banner("0.1.0", &legs, &[], None, None);
        assert!(
            banner.warnings.is_empty(),
            "a normal build must produce no warning rows: {:?}",
            banner.warnings
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

/// The live `[server] tailscale` mode, driven through the real serve loop with
/// the detector injected and no Tailscale binary anywhere.
///
/// The Tailscale leg is stood in for by a second loopback address: `desired_leg`
/// only refuses an address the primary already covers, so `127.0.0.2` is a leg
/// like any other and binds for real, which is what makes "did the listener
/// actually move" a fact rather than a claim about a cell.
#[cfg(test)]
mod live_tailscale_mode_tests {
    use super::*;
    use dux_core::tailscale::TailscaleUnavailable;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    /// Bounds every wait in this module. Long enough that a loaded machine never
    /// trips it, short enough that a regression fails instead of hanging.
    const WAIT: Duration = Duration::from_secs(5);

    /// The stand-in Tailscale address.
    fn leg_ip() -> IpAddr {
        "127.0.0.2".parse().unwrap()
    }

    /// A primary listener held open for the whole test, so its port stays
    /// reserved on `127.0.0.1` while the leg binds the same port on `127.0.0.2`.
    fn primary_listener() -> (std::net::TcpListener, SocketAddr) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free loopback port");
        let addr = listener.local_addr().expect("its address");
        (listener, addr)
    }

    struct Harness {
        shutdown: ServeShutdown,
        control: TailscaleModeControl,
        bound: Arc<std::sync::Mutex<Option<SocketAddr>>>,
        legs: tokio::sync::mpsc::Sender<(u64, LegCommand)>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Harness {
        fn start(
            mode: TailscaleMode,
            forced_no: bool,
            primary: Option<SocketAddr>,
            initial_leg: Option<SocketAddr>,
            detect: Arc<dyn Fn() -> Result<IpAddr, TailscaleUnavailable> + Send + Sync>,
        ) -> Self {
            let (control, mode_rx) = TailscaleModeControl::new(
                tokio::runtime::Handle::current(),
                Arc::new(AtomicBool::new(mode.watches_interface())),
                Arc::new(AtomicBool::new(mode.wants_tailscale())),
            );
            let shutdown = ServeShutdown::new(control.watched());
            if let Some(addr) = initial_leg {
                // A live leg the loop can genuinely stop, so "it unbound" is the
                // registry answering rather than a cell being blanked.
                let _ = shutdown.register_leg(addr);
            }
            let mut ts =
                TailscaleLoop::new(mode, forced_no, primary, initial_leg, &control, detect);
            let leg_rx = ts.take_leg_receiver();
            let bound = ts.bound_cell();
            let legs = ts.leg_sender();
            ts.start_watcher_if_wanted();
            let task = tokio::spawn(run_serve_loop(
                tokio::task::JoinSet::new(),
                shutdown.clone(),
                leg_rx,
                mode_rx,
                axum::Router::new(),
                Console::noop(),
                ts,
            ));
            Self {
                shutdown,
                control,
                bound,
                legs,
                task,
            }
        }

        fn bound(&self) -> Option<SocketAddr> {
            *self.bound.lock().expect("the cell is not poisoned")
        }

        async fn finish(self) {
            self.shutdown.trigger();
            tokio::time::timeout(WAIT, self.task)
                .await
                .expect("a tripped lane must end the serve loop")
                .expect("the serve loop task joins");
        }
    }

    #[tokio::test]
    async fn switching_to_no_drops_the_leg_stops_the_watcher_and_closes_the_host_rule() {
        let (_primary, primary_addr) = primary_listener();
        let leg = SocketAddr::new(leg_ip(), primary_addr.port());
        let h = Harness::start(
            TailscaleMode::Auto,
            false,
            Some(primary_addr),
            Some(leg),
            Arc::new(|| Ok("127.0.0.2".parse().unwrap())),
        );
        assert!(
            h.control.watched().load(Ordering::SeqCst),
            "auto starts a watcher"
        );

        let outcome = h.control.set_mode(TailscaleMode::No).await;
        assert_eq!(outcome, TailscaleModeOutcome::Detached);
        assert_eq!(h.bound(), None, "the leg is gone");
        assert!(
            !h.control.watched().load(Ordering::SeqCst),
            "and nothing is watching for it any more"
        );
        assert!(
            !h.control.host_literals().load(Ordering::SeqCst),
            "the Host guard must stop admitting tailnet literals with the leg"
        );
        h.finish().await;
    }

    #[tokio::test]
    async fn switching_to_yes_binds_what_the_detection_found() {
        let (_primary, primary_addr) = primary_listener();
        let h = Harness::start(
            TailscaleMode::No,
            false,
            Some(primary_addr),
            None,
            Arc::new(|| Ok(leg_ip())),
        );

        let outcome = h.control.set_mode(TailscaleMode::Yes).await;
        let expected = SocketAddr::new(leg_ip(), primary_addr.port());
        assert_eq!(
            outcome,
            TailscaleModeOutcome::Applied {
                bound: Some(expected)
            }
        );
        assert_eq!(h.bound(), Some(expected));
        assert!(
            h.shutdown.has_leg(expected),
            "a real listener must be serving there"
        );
        assert!(
            h.control.host_literals().load(Ordering::SeqCst),
            "and the Host guard must admit tailnet literals again"
        );
        assert!(
            !h.control.watched().load(Ordering::SeqCst),
            "yes looks once; nothing keeps watching"
        );
        h.finish().await;
    }

    #[tokio::test]
    async fn switching_to_yes_with_no_address_warns_and_binds_nothing() {
        let (_primary, primary_addr) = primary_listener();
        let h = Harness::start(
            TailscaleMode::No,
            false,
            Some(primary_addr),
            None,
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
        );

        assert_eq!(
            h.control.set_mode(TailscaleMode::Yes).await,
            TailscaleModeOutcome::NothingDetected
        );
        assert_eq!(h.bound(), None);
        h.finish().await;
    }

    #[tokio::test]
    async fn switching_to_auto_starts_a_watcher_that_probes_at_once() {
        let (_primary, primary_addr) = primary_listener();
        let (probed_tx, probed_rx) = std::sync::mpsc::channel();
        let h = Harness::start(
            TailscaleMode::No,
            false,
            Some(primary_addr),
            None,
            Arc::new(move || {
                let _ = probed_tx.send(());
                Err(TailscaleUnavailable::NoAddress)
            }),
        );

        assert_eq!(
            h.control.set_mode(TailscaleMode::Auto).await,
            TailscaleModeOutcome::Applied { bound: None },
            "nothing is bound yet, and dux is watching for it"
        );
        assert!(h.control.watched().load(Ordering::SeqCst));
        probed_rx
            .recv_timeout(WAIT)
            .expect("the watcher must probe before its first park, not ten seconds later");
        h.finish().await;
    }

    #[tokio::test]
    async fn switching_from_yes_to_auto_keeps_the_leg_that_is_already_serving() {
        let (_primary, primary_addr) = primary_listener();
        let leg = SocketAddr::new(leg_ip(), primary_addr.port());
        let h = Harness::start(
            TailscaleMode::Yes,
            false,
            Some(primary_addr),
            Some(leg),
            // The interface is still there, so the watcher's first probe plans
            // nothing and the listener is left exactly where it was.
            Arc::new(move || Ok(leg_ip())),
        );

        assert_eq!(
            h.control.set_mode(TailscaleMode::Auto).await,
            TailscaleModeOutcome::Applied { bound: Some(leg) }
        );
        assert_eq!(h.bound(), Some(leg), "the leg is untouched");
        h.finish().await;
    }

    #[tokio::test]
    async fn a_run_started_with_no_tailscale_refuses_and_changes_nothing() {
        let (_primary, primary_addr) = primary_listener();
        let h = Harness::start(
            TailscaleMode::No,
            true,
            Some(primary_addr),
            None,
            Arc::new(|| Ok(leg_ip())),
        );

        for mode in [TailscaleMode::Auto, TailscaleMode::Yes] {
            assert_eq!(
                h.control.set_mode(mode).await,
                TailscaleModeOutcome::RefusedForcedNo
            );
        }
        assert_eq!(h.bound(), None, "nothing was bound behind the refusal");
        assert!(!h.control.watched().load(Ordering::SeqCst));
        assert!(
            !h.control.host_literals().load(Ordering::SeqCst),
            "and the Host guard stayed closed"
        );
        h.finish().await;
    }

    #[tokio::test]
    async fn the_leg_command_lane_stays_open_in_the_static_modes() {
        // The loop holds a sender for its whole life, so a mode with no watcher
        // behind it is not a closed channel: switching back to `auto` later must
        // find the arm still listening.
        let (_primary, primary_addr) = primary_listener();
        let leg = SocketAddr::new(leg_ip(), primary_addr.port());
        let h = Harness::start(
            TailscaleMode::Yes,
            false,
            Some(primary_addr),
            None,
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
        );
        assert!(
            !h.control.watched().load(Ordering::SeqCst),
            "yes starts no watcher thread"
        );

        // Generation 0 is what a serve that never started a watcher is on.
        h.legs
            .send((0, LegCommand::Bind(leg)))
            .await
            .expect("the lane must still be open");
        let bound = tokio::time::timeout(WAIT, async {
            loop {
                if h.bound() == Some(leg) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(bound.is_ok(), "the loop must still act on leg commands");
        h.finish().await;
    }

    #[tokio::test]
    async fn a_command_from_a_replaced_watcher_generation_is_dropped() {
        // A watcher parked in a probe when the mode changed comes back with a
        // command for the mode dux already left. Acting on it would re-bind the
        // leg the change just let go.
        let (_primary, primary_addr) = primary_listener();
        let stale = SocketAddr::new("127.0.0.3".parse::<IpAddr>().unwrap(), primary_addr.port());
        let current = SocketAddr::new(leg_ip(), primary_addr.port());
        // A detection that never answers, so neither watcher thread emits
        // anything of its own and the only commands the loop sees are this
        // test's.
        let (release, parked) = std::sync::mpsc::channel::<()>();
        let parked = std::sync::Mutex::new(parked);
        let h = Harness::start(
            TailscaleMode::Auto,
            false,
            Some(primary_addr),
            None,
            Arc::new(move || {
                let _ = parked.lock().expect("not poisoned").recv();
                Err(TailscaleUnavailable::NoAddress)
            }),
        );

        // The startup watcher is generation 1; this change replaces it with 2.
        assert_eq!(
            h.control.set_mode(TailscaleMode::Auto).await,
            TailscaleModeOutcome::Applied { bound: None }
        );

        // One lane, so these arrive in order: once the current-generation command
        // has been acted on, the stale one has already been decided.
        h.legs.send((1, LegCommand::Bind(stale))).await.unwrap();
        h.legs.send((2, LegCommand::Bind(current))).await.unwrap();
        let landed = tokio::time::timeout(WAIT, async {
            loop {
                if h.bound() == Some(current) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(landed.is_ok(), "the current generation's command must land");
        assert!(
            !h.shutdown.has_leg(stale),
            "a stale generation's bind must never have been served"
        );
        drop(release);
        h.finish().await;
    }

    #[tokio::test]
    async fn a_watcher_command_from_before_the_switch_to_no_is_dropped() {
        // A watcher whose probe returned in the window between the switch to
        // `no` storing its stop flag and the loop reading the next command is
        // still allowed to emit. Its bind must not put the leg back.
        let (_primary, primary_addr) = primary_listener();
        let leg = SocketAddr::new(leg_ip(), primary_addr.port());
        let stale = SocketAddr::new("127.0.0.3".parse::<IpAddr>().unwrap(), primary_addr.port());
        // A detection that never answers, so the only commands the loop sees are
        // this test's.
        let (release, parked) = std::sync::mpsc::channel::<()>();
        let parked = std::sync::Mutex::new(parked);
        let h = Harness::start(
            TailscaleMode::Auto,
            false,
            Some(primary_addr),
            Some(leg),
            Arc::new(move || {
                let _ = parked.lock().expect("not poisoned").recv();
                Err(TailscaleUnavailable::NoAddress)
            }),
        );

        assert_eq!(
            h.control.set_mode(TailscaleMode::No).await,
            TailscaleModeOutcome::Detached
        );

        // Generation 1 is the startup watcher's, which `no` stopped.
        h.legs.send((1, LegCommand::Bind(stale))).await.unwrap();
        // One lane, so once this has been acted on the stale one has already
        // been decided.
        h.legs.send((2, LegCommand::Bind(leg))).await.unwrap();
        let landed = tokio::time::timeout(WAIT, async {
            loop {
                if h.bound() == Some(leg) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            landed.is_ok(),
            "stopping a watcher must move the generation on, so the next one is current"
        );
        assert!(
            !h.shutdown.has_leg(stale),
            "a watcher stopped by the switch to no must not re-bind the leg"
        );
        drop(release);
        h.finish().await;
    }

    /// The probe the loop calls, boxed so a test can script it.
    type Detector = Arc<dyn Fn() -> Result<IpAddr, TailscaleUnavailable> + Send + Sync>;

    /// A detector the test can hold parked, plus the signal that it started.
    fn parked_detector() -> (
        Detector,
        tokio::sync::mpsc::UnboundedReceiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        // The "it started" signal is a tokio channel and the park is a std one:
        // the test AWAITS the first (blocking on it would stop the runtime the
        // serve loop is on) and the detector BLOCKS on the second, which is the
        // whole point of running it off the loop.
        let (started_tx, started_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Mutex::new(release_rx);
        let detect: Detector = Arc::new(move || {
            let _ = started_tx.send(());
            let _ = release_rx.lock().expect("not poisoned").recv();
            Err(TailscaleUnavailable::NoAddress)
        });
        (detect, started_rx, release_tx)
    }

    #[tokio::test]
    async fn a_superseded_request_is_answered_rather_than_left_hanging() {
        let (_primary, primary_addr) = primary_listener();
        let (detect, mut started, release) = parked_detector();
        let h = Harness::start(TailscaleMode::No, false, Some(primary_addr), None, detect);

        let control = h.control.clone();
        let first = tokio::spawn(async move { control.set_mode(TailscaleMode::Yes).await });
        tokio::time::timeout(WAIT, started.recv())
            .await
            .expect("the detection must have started");

        assert_eq!(
            h.control.set_mode(TailscaleMode::No).await,
            TailscaleModeOutcome::Applied { bound: None },
            "the newer request is carried out"
        );
        let superseded = tokio::time::timeout(WAIT, first)
            .await
            .expect("the older request must be answered, not dropped")
            .expect("the request task joins");
        assert_eq!(superseded, TailscaleModeOutcome::Superseded);
        drop(release);
        h.finish().await;
    }

    #[tokio::test]
    async fn a_parked_detection_does_not_stop_the_parent_lane_from_ending_the_loop() {
        // The detection is its own select arm precisely so a five-second probe
        // cannot hold a teardown, a leg death or a watcher command behind it.
        let (_primary, primary_addr) = primary_listener();
        let (detect, mut started, release) = parked_detector();
        let h = Harness::start(TailscaleMode::No, false, Some(primary_addr), None, detect);

        let control = h.control.clone();
        let pending = tokio::spawn(async move { control.set_mode(TailscaleMode::Yes).await });
        tokio::time::timeout(WAIT, started.recv())
            .await
            .expect("the detection must have started");

        h.shutdown.trigger();
        tokio::time::timeout(WAIT, h.task)
            .await
            .expect("a parked detection must not hold the teardown")
            .expect("the serve loop task joins");
        let answered = tokio::time::timeout(WAIT, pending)
            .await
            .expect("the pending request must be answered as the loop ends")
            .expect("the request task joins");
        assert_eq!(answered, TailscaleModeOutcome::NotServing);
        drop(release);
    }

    #[tokio::test]
    async fn a_closed_mode_lane_leaves_the_rest_of_the_loop_working() {
        // Every control handle is gone, so no mode change can arrive again. The
        // loop keeps serving: the legs, the parent lane and the teardown all
        // still work, and the mode arm is simply disabled rather than left
        // resolving instantly over a closed receiver.
        let (_primary, primary_addr) = primary_listener();
        let h = Harness::start(
            TailscaleMode::No,
            false,
            Some(primary_addr),
            None,
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
        );
        let Harness {
            shutdown,
            control,
            legs,
            task,
            ..
        } = h;
        drop(control);

        // Ordinary work, on the same runtime, after the lane closed.
        let leg = SocketAddr::new(leg_ip(), primary_addr.port());
        legs.send((0, LegCommand::Bind(leg)))
            .await
            .expect("the leg lane is still open");
        tokio::time::timeout(WAIT, async {
            loop {
                if shutdown.has_leg(leg) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a closed mode lane must not starve the rest of the loop");

        shutdown.trigger();
        tokio::time::timeout(WAIT, task)
            .await
            .expect("a tripped lane must end the serve loop")
            .expect("the serve loop task joins");
    }

    #[tokio::test]
    async fn a_control_whose_loop_is_gone_reports_that_nothing_is_serving() {
        let (control, mode_rx) = TailscaleModeControl::new(
            tokio::runtime::Handle::current(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        drop(mode_rx);
        assert_eq!(
            control.set_mode(TailscaleMode::Auto).await,
            TailscaleModeOutcome::NotServing
        );
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
            // so the test exercises the guarded completion path rather than a bare send.
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
