//! The web layer: exposes the `dux-core` engine over HTTP/WebSocket so a browser
//! SPA can drive the same agent sessions the TUI does.
//!
//! Entry points:
//!
//! - [`run_server`] boots the engine on its own thread and serves axum on a
//!   self-built tokio runtime until SIGINT or SIGTERM.
//! - [`serve_with_engine`] serves over an EXISTING live engine, PTYs intact, on the
//!   caller's thread, returning the engine when serving stops so the TUI can resume
//!   around the same agents.
//!
//! [`server`] holds the axum router and the same-origin WebSocket check;
//! [`engine_actor`] holds the `EngineHandle` and the loop that owns the `!Send`
//! engine on its thread.
//!
//! This crate depends on `dux-core`, never `dux-tui`. The `dep-isolation` CI job
//! runs `cargo tree -p dux-web` and fails if any TUI-only crate appears.

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
pub mod pty_log;
/// The PTY input-ownership registry, re-exported from `dux-core`: a rule two
/// surfaces obey belongs in the crate both can see. A glob rather than a named
/// list, because some names are used only by this crate's test modules and naming
/// them would be an unused import in a normal build.
pub(crate) mod pty_owners {
    pub(crate) use dux_core::pty_owners::*;
}
pub(crate) mod pty_sizes;
pub(crate) mod reload_signal;
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

use std::io::IsTerminal;
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

use crate::console::Console;
use crate::engine_actor::LoopControl;
use crate::serve_legs::{
    LegCommand, LegStatus, LegStep, ModeStep, ServeShutdown, StartupLeg, TailscaleModeControl,
    WATCH_PERIOD, Watch, WatchEvent, admitted_own_name, desired_leg, identity_news, plan_leg_step,
    plan_mode_change, serve_hint, tailnet_urls, wait_for_leg_shutdown, waiting_note,
    watch_tailscale,
};
use crate::server::RouterParams;
use dux_core::serve_log::{Banner, ListenerRow, StartupNotes};
use dux_core::tailscale::TailscaleIdentity;

/// Boot the engine on its own thread and serve the web UI on every address in
/// the plan (one axum task per listener, sharing the router/state). Blocking
/// entry: it builds its own tokio runtime.
///
/// `version` is the dux crate version the binary passes in (`CARGO_PKG_VERSION`)
/// for the console banner header.
///
/// The stdout [`Console`] is built here from the engine's loaded
/// `[server] color`/`access_log` and threaded into the serve paths. The TUI flip
/// ([`serve_with_engine`]) builds a capturing console instead, because its
/// status screen owns the terminal, and records the very same lines.
///
/// `startup_warnings` are the warnings the caller raised before anything was
/// loaded (Tailscale detection, a non-loopback bind). They print as the first
/// lines of the log, before any address is bound, so they are on screen even if
/// a bind then fails.
pub fn run_server(
    paths: DuxPaths,
    plan: ServerPlan,
    version: String,
    startup_warnings: Vec<StartupWarning>,
) -> Result<()> {
    run_plain_http(paths, plan, version, startup_warnings)
}

/// A warning `dux server` raised before anything was loaded, for the head of
/// its log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupWarning {
    pub text: String,
    /// Already printed to stderr by the caller (the non-loopback alarm, printed
    /// the moment it is known), so neither the log's stderr echo nor a failed
    /// load prints it there again.
    pub already_on_stderr: bool,
}

/// Log a WARN when this binary has no web UI compiled in (built with
/// `DUX_DISABLE_UI_BUILD` and no previously built `web/dist`).
///
/// The message lands in THREE places on purpose, because the two audiences are
/// different people who look in different places:
/// - the served page itself (build.rs's notice page), for whoever opens a browser,
///   possibly on a phone, with no access to this terminal;
/// - the startup banner (a ⚠ row), for the operator who launched it and can
///   rebuild: `dux server`'s terminal, or the flip's log viewer, which shows the
///   same banner;
/// - `dux.log`.
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
/// Returns `(console, access_log)`. An unrecognized color value is warned about
/// on the console it built, as its first line. The flip does NOT call this: the
/// setting governs `dux server`'s stdout only, and the flip's viewer is themed.
fn build_console(config: &dux_core::config::Config) -> (Console, bool) {
    let setting = &config.server.color;
    let color = crate::console::detect(setting);
    let console = Console::stdout(color, dux_core::serve_log::StdStreams::current());
    // dux.log already has it: the load logs every value it reads as another.
    if !crate::console::is_known_color_setting(setting) {
        console.warn(&crate::console::unknown_color_warning(setting));
    }
    // Only on a terminal: a QR code in a piped log is a screenful of blocks
    // nobody can scan.
    console.set_qr_codes(config.server.qr_codes && std::io::stdout().is_terminal());
    (console, config.server.access_log)
}

/// The warning shown when a BEST-EFFORT (Tailscale) listener cannot bind because
/// something else already holds that address. Names the address, the cause, and
/// BOTH remedies (stop the other process, or change the port). Emitted as a
/// `dux.log` WARN line. Pure so it is unit-testable.
fn tailscale_bind_warning(addr: SocketAddr, err: &std::io::Error) -> String {
    dux_core::serve_log::tailscale_bind_warning(addr, err)
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
///   failure is FATAL: it logs a `logger::error` with the failing address and
///   returns the error (with address context) so the serve aborts. This is the
///   explicit-failure tenet: the operator named this address.
/// - BEST-EFFORT (the Tailscale leg of LOCAL MODE): a bind failure logs a WARN
///   naming the address, the cause, and both remedies, collects the SAME text in
///   the returned warnings vec, and CONTINUES without that listener.
///
/// If NOTHING binds (every address failed) the whole serve is fatal: there is
/// nothing left to serve. Returns the bound listeners (with their addresses) and
/// the best-effort warnings (the caller logs them to `dux.log`; they are not
/// re-broadcast, and [`run_plain_http`] explains why a startup broadcast reaches
/// no clients). The returned vec is retained because the bind tests assert on it.
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
                    "[server] could not bind the listen address {addr}: {err}. Something else \
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
    let listeners: Vec<ListenerRow> = bound
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

/// The startup banner every serving mode prints, built from the legs that bound
/// (each an `(addr, required)` pair). The one recipe `dux server` and the flip
/// share, so the two print the same rows for the same situation: the bind
/// warnings, the `auto` mode's waiting note, the reachability note and the
/// missing-web-UI warning all come from here.
///
/// `tailscale_detected` is whether a Tailscale address was found before binding,
/// which is what tells "the address would not bind" from "there is no address
/// yet" when no Tailscale leg is in `bound`.
pub(crate) fn serve_banner(
    version: &str,
    bound: &[(SocketAddr, bool)],
    bind_warnings: &[String],
    tailscale: TailscaleMode,
    tailscale_detected: bool,
) -> Banner {
    let bound_plan_addrs: Vec<PlanAddr> = bound
        .iter()
        .map(|(addr, required)| {
            if *required {
                PlanAddr::required(*addr)
            } else {
                PlanAddr::best_effort(*addr)
            }
        })
        .collect();
    let leg_bound = bound
        .iter()
        .any(|(addr, required)| !required && !addr.ip().is_loopback());
    let startup_leg = match (tailscale_detected, leg_bound) {
        (_, true) => StartupLeg::Bound,
        (true, false) => StartupLeg::BindFailed,
        (false, false) => StartupLeg::Undetected,
    };
    let note = safety_note(&bound_plan_addrs, tailscale);
    let mut warnings = bind_warnings.to_vec();
    warnings.extend(waiting_note(tailscale, startup_leg));
    plain_http_banner(
        version,
        bound,
        &warnings,
        note,
        web_assets::ui_startup_warning(),
    )
}

/// How far the server can be reached, classified from the BOUND legs (each an
/// `(addr, required)` pair where `required` is true for explicit `--bind`
/// public/LAN entries and false for best-effort Tailscale local-mode legs).
/// Worst-wins: any required non-loopback leg makes it `Public`; otherwise any
/// best-effort non-loopback leg makes it `Tailscale`; otherwise `LoopbackOnly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reachability {
    /// Every bound leg is genuine loopback: nothing off-host can reach it.
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
/// primary + a best-effort Tailscale leg), as the banner's reachability row in
/// every serving mode.
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
/// `dux server` and the status screen's log viewer for the flip.
fn run_plain_http(
    paths: DuxPaths,
    plan: ServerPlan,
    version: String,
    startup_warnings: Vec<StartupWarning>,
) -> Result<()> {
    let ServerPlan {
        addrs,
        primary,
        tailscale,
        forced_no,
    } = plan;
    warn_if_ui_not_built();
    let engine = bootstrap_or_report(&paths, &startup_warnings, &mut std::io::stderr())?;
    // Build the vite-style CLI console (color from [server] color) + the access-log
    // toggle before the engine moves into the actor thread.
    let (console, access_log) = build_console(&engine.config);
    // What the caller learned before anything was loaded prints first, ahead of
    // the bind, so it is on screen even if a bind then fails.
    for warning in &startup_warnings {
        if warning.already_on_stderr {
            console.warn_already_on_stderr(&warning.text);
        } else {
            console.warn(&warning.text);
        }
    }
    // A second stop signal and the engine's shutdown share this, so a forced
    // quit kills the children and logs what the flip logs.
    let quit = Arc::new(QuitForce::default());
    // Held past the serve so the run's last lines (its shutdown) are flushed to
    // the terminal before the process exits.
    let exit_console = console.clone();
    // The router's whole `[server]` input, snapshotted before the engine moves
    // into the actor thread. One clone rather than a field-by-field capture, so
    // this path and the two in-app ones derive their router from the same
    // `router_params` recipe and a new limit cannot reach two of the three.
    let router_config = engine.config.clone();
    // The engine thread's handle, kept so the run joins it (the engine drops and
    // its queued config writes land) before the process ends.
    let engine_thread: Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>> = Arc::default();
    flushing(&exit_console, || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let engine_thread_slot = Arc::clone(&engine_thread);
        let quit_for_serve = Arc::clone(&quit);
        let result = runtime.block_on(async move {
            let quit = quit_for_serve;
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
            let initial_tailscale_leg = bound
                .iter()
                .find(|b| !b.required && !b.addr.ip().is_loopback())
                .map(|b| b.addr);
            // The PLAN says whether an address was detected (a best-effort address
            // is only in it when one was); `bound` says whether it bound.
            console.banner(&serve_banner(
                &version,
                &banner_legs,
                &bind_warnings,
                tailscale,
                addrs.iter().any(|p| !p.is_required()),
            ));

            // Spawn the engine on its own std thread (it runs the synchronous engine
            // loop, not a tokio task). Its shutdown progress prints on this console.
            let (handle, join) = engine_actor::spawn_engine_thread_with_console(
                engine,
                console.clone(),
                Arc::clone(&quit),
            );
            *engine_thread_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(join);

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
                .with_live_own_magicdns_name(mode_control.own_magicdns_name())
                .with_live_funnel_lockout(mode_control.funnel_lockout())
                .with_tailscale_mode_control(mode_control.clone(), forced_no),
            );

            // Translate a SIGINT/SIGTERM into a watch trip so every listener winds
            // down gracefully (the same trigger a first-listener failure uses).
            {
                let shutdown = shutdown.clone();
                let force_exit = ForceExit::new(console.clone(), None, Arc::clone(&quit));
                tokio::spawn(async move {
                    shutdown_signal(force_exit).await;
                    shutdown.trigger();
                });
            }
            // SIGUSR1 (`dux config set`, or `kill -USR1`) runs the engine
            // actor's reload, until the serve stops.
            tokio::spawn(reload_signal::reload_on_signal(
                handle.clone(),
                shutdown.subscribe(),
            ));

            // Serve every BOUND address, each on its own leg (its own stop lane), so
            // the Tailscale leg can be added and dropped later without disturbing the
            // required one.
            //
            // The leg's news goes to the surfaces as well as to the console: the
            // console is this process's terminal, and a browser reaching dux over
            // the tailnet is exactly the client that loses the address.
            let leg_status = LegStatus::new(handle.clone());
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
                    leg_status.clone(),
                );
            }

            // This machine's tailnet name, its `tailscale serve` routes and its
            // Funnel state, read AFTER every listener is serving and on a
            // blocking thread, never on this future's own: the CLI calls are
            // bounded but can take seconds on a wedged daemon, and serving must
            // not wait on them. Every listener answers 503 until this lands (the
            // lockout starts in its checking state on every mode but `no`),
            // which is the fail-closed reading of "nobody has looked yet".
            let identity_port = primary.port();
            let first_look = if tailscale.wants_tailscale() {
                Some(
                    tokio::task::spawn_blocking(move || {
                        dux_core::tailscale::detect_identity(identity_port)
                    })
                    .await
                    .unwrap_or(Err(TailscaleUnavailable::CommandFailed)),
                )
            } else {
                None
            };

            // On `auto`, watch the Tailscale interface for the rest of the run.
            let mut tailscale_loop = TailscaleLoop::new(
                tailscale,
                forced_no,
                Some(primary),
                initial_tailscale_leg,
                &mode_control,
                Arc::new(dux_core::tailscale::detect_ip),
            )
            .with_identity(
                Arc::new(move || dux_core::tailscale::detect_identity(identity_port)),
                None,
            );
            // The first look's Funnel verdict and its warnings, before the
            // watcher starts, so a look that succeeded leaves it parked rather
            // than looking again at once. The tailnet rows and QR codes follow
            // on the serve loop's first turn.
            if let Some(look) = first_look {
                tailscale_loop.apply_look(look, &console, &leg_status);
            }
            let leg_commands = tailscale_loop.take_leg_receiver();
            tailscale_loop.start_watcher_if_wanted();
            tailscale_loop.say_not_checking(&console);
            tailscale_loop.say_serve_hint_once(&console);

            run_serve_loop(
                tasks,
                shutdown.clone(),
                leg_commands,
                mode_requests,
                app,
                console.clone(),
                leg_status,
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
        });
        let join = engine_thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(join) = join {
            match finish_engine_thread(join, &quit, &exit_console, FORCE_SETTLE_BOUND) {
                OwnerExit::Normal => {}
                // A second signal forced the quit and this thread took the exit:
                // the engine is dropped and the log flushed, so it can go.
                OwnerExit::Forced => std::process::exit(130),
                // The hatch took the exit because this thread was late; it is
                // exiting now, and a second exit must not race it.
                OwnerExit::HatchExiting => loop {
                    std::thread::park();
                },
            }
        }
        result
    })
}

/// Run `body`, then flush `console` (bounded) whichever way it returned, so the
/// lines logged before an early failure still reach their stream.
pub(crate) fn flushing<T>(console: &Console, body: impl FnOnce() -> Result<T>) -> Result<T> {
    let result = body();
    console.flush();
    result
}

/// How the engine's owner ends a `dux server` run, once the serve is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerExit {
    /// No forced quit: return normally.
    Normal,
    /// A forced quit, and this thread took the exit: exit 130.
    Forced,
    /// A forced quit, and the signal hatch took the exit first: it is exiting,
    /// so this thread must not call exit again.
    HatchExiting,
}

/// Join the engine thread, which drops the engine and so lands its queued
/// config writes, waiting at most `bound` for it; flush the log; then say how
/// the run ends. The owner side of the hand-over a second stop signal makes:
/// the hatch leaves the exit to whoever drops the engine, unless that takes
/// longer than the bound.
pub(crate) fn finish_engine_thread(
    join: std::thread::JoinHandle<()>,
    quit: &QuitForce,
    console: &Console,
    bound: Duration,
) -> OwnerExit {
    let deadline = std::time::Instant::now() + bound;
    while !join.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if join.is_finished() {
        let _ = join.join();
    }
    console.flush();
    if !quit.is_requested() {
        OwnerExit::Normal
    } else if quit.claim_exit() {
        OwnerExit::Forced
    } else {
        OwnerExit::HatchExiting
    }
}

/// Load the engine for `dux server`, and when that fails, print the startup
/// warnings to `err` before giving up: they were going to open the log, and a
/// start that never gets as far as the log must not swallow them (the
/// non-loopback alarm least of all).
pub(crate) fn bootstrap_or_report(
    paths: &DuxPaths,
    startup_warnings: &[StartupWarning],
    err: &mut dyn std::io::Write,
) -> Result<Engine> {
    bootstrap::bootstrap_engine(paths).inspect_err(|_| {
        for warning in startup_warnings.iter().filter(|w| !w.already_on_stderr) {
            let _ = writeln!(err, "WARNING: {}", warning.text);
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
    /// A second Ctrl-c during the quit's shutdown wait cut it short: the
    /// children were killed rather than waited for. The caller gives the
    /// terminal back and hands the handle to [`finish_forced_quit`], which
    /// drops the engine and exits with 130 unless the signal hatch already did.
    ForceQuit(ForceQuitHandle),
}

/// The flip's forced quit, carried to the binary so it and the signal hatch
/// agree on which of them exits.
#[derive(Clone, Default)]
pub struct ForceQuitHandle(pub(crate) Arc<QuitForce>);

/// What the flip's status screen lends the serve, when it is on screen. Both are
/// `None` without one (no TTY), and then there is nothing to restore and Ctrl-c
/// is an ordinary signal.
#[derive(Default)]
pub struct FlipHooks {
    /// Gives the terminal back before a second stop SIGNAL forces the process
    /// out, because that exit runs no destructor.
    pub restore_terminal: Option<RestoreTerminal>,
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
#[allow(clippy::too_many_arguments)]
fn spawn_leg(
    tasks: &mut tokio::task::JoinSet<()>,
    app: Router,
    listener: tokio::net::TcpListener,
    addr: SocketAddr,
    required: bool,
    shutdown: &ServeShutdown,
    console: Console,
    status: LegStatus,
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
                let warning = shutdown.record_best_effort_failure(addr, &err);
                console.bind_degraded(&format!(
                    "the Tailscale listener on {addr} stopped serving: {err}"
                ));
                // The console is `dux server`'s terminal and nothing else, so
                // the surfaces get the fuller sentence that names the way back.
                status.degraded(&warning);
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
    leg_tx: tokio::sync::mpsc::Sender<(u64, WatchEvent)>,
    /// Taken once by the serve path and handed to the loop.
    leg_rx: Option<tokio::sync::mpsc::Receiver<(u64, WatchEvent)>>,
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
    /// The name probe: this machine's MagicDNS name and the `tailscale serve`
    /// routes that end at dux. Injected for the same reason as `detect`.
    identify: IdentityProbe,
    /// The identity this serve holds. Written only here; the URL lists read it.
    identity: crate::serve_legs::IdentityCell,
    /// The name the Host guard admits (rule 6). Written only here.
    own_name: crate::host_guard::LiveHostNames,
    /// Whether the one-time `tailscale serve` tip has been given this run.
    serve_hint_given: bool,
    /// The URLs the QR codes last showed, so they are shown again only when
    /// those change. `None` until the loop's first look.
    last_qr: Option<Vec<String>>,
    /// The URL list last handed to the surfaces, for the same reason.
    last_urls: Option<Vec<String>>,
    /// The tailnet address rows last printed, for the same reason.
    last_rows: Vec<ListenerRow>,
    /// Whether the held identity is CURRENT: set by a successful look, cleared
    /// by a failed one. While it is clear the Host guard admits no name, and
    /// the watcher treats the identity as unknown, so the next successful look
    /// is sent (and re-admits the name) even when nothing about it changed.
    identity_confirmed: Arc<AtomicBool>,
    /// How long a watcher parks between looks. [`WATCH_PERIOD`] outside tests.
    watch_period: Duration,
    /// Whether the Host guard serves at all, as far as Funnel goes. Written
    /// only here, from what each look at Tailscale found.
    funnel_lockout: crate::host_guard::FunnelLockoutCell,
    /// Whether dux runs inside a container, where a Tailscale outside it is
    /// invisible. Read once; injected by tests.
    in_container: bool,
    /// Whether any look has landed yet, so the container warning is said at
    /// start or not at all.
    first_look_landed: bool,
    /// Test-only: refuse to start watcher threads, standing in for a system
    /// that cannot spawn one.
    #[cfg(test)]
    refuse_watcher_threads: bool,
}

/// The injected name probe, see [`TailscaleLoop::with_identity`].
pub(crate) type IdentityProbe =
    Arc<dyn Fn() -> Result<TailscaleIdentity, TailscaleUnavailable> + Send + Sync>;

/// A one-shot detection the loop is waiting on, kept OUT of the mode arm so the
/// parent lane, leg deaths and watcher commands keep flowing while it runs.
struct PendingDetect {
    /// The generation this detection belongs to. A newer request bumps the
    /// counter, and the answer of an older one is discarded.
    generation: u64,
    reply: tokio::sync::oneshot::Sender<TailscaleModeOutcome>,
    task: tokio::task::JoinHandle<OneShotLook>,
}

/// What a one-shot `yes` detection found: the address, and the name.
type OneShotLook = (
    Result<IpAddr, TailscaleUnavailable>,
    Result<TailscaleIdentity, TailscaleUnavailable>,
);

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
            identify: Arc::new(|| Err(TailscaleUnavailable::CommandMissing)),
            identity: control.identity(),
            own_name: control.own_magicdns_name(),
            serve_hint_given: false,
            last_qr: None,
            last_urls: None,
            last_rows: Vec::new(),
            identity_confirmed: Arc::new(AtomicBool::new(false)),
            watch_period: WATCH_PERIOD,
            funnel_lockout: control.funnel_lockout(),
            // Unit tests default to a plain host, so a suite run inside a
            // container is not told about it by every test.
            in_container: !cfg!(test) && dux_core::container::running_in_container(),
            first_look_landed: false,
            #[cfg(test)]
            refuse_watcher_threads: false,
        }
    }

    /// Say whether dux runs inside a container, instead of asking the system.
    #[cfg(test)]
    pub(crate) fn in_container(mut self, in_container: bool) -> Self {
        self.in_container = in_container;
        self
    }

    /// Fail every watcher start, as a system that cannot spawn a thread would.
    #[cfg(test)]
    pub(crate) fn refusing_watcher_threads(mut self) -> Self {
        self.refuse_watcher_threads = true;
        self
    }

    /// Look every `period` instead of every [`WATCH_PERIOD`], so a test of what
    /// a later look finds does not wait seconds for it.
    #[cfg(test)]
    pub(crate) fn with_watch_period(mut self, period: Duration) -> Self {
        self.watch_period = period;
        self
    }

    /// Give the loop its name probe and, when the serve path already looked,
    /// the identity it found, which the Host guard admits before the first
    /// request arrives. Without this the loop never learns a name, which is what
    /// a test that is not about names wants.
    pub(crate) fn with_identity(
        mut self,
        identify: IdentityProbe,
        initial: Option<TailscaleIdentity>,
    ) -> Self {
        self.identify = identify;
        if let Some(identity) = initial {
            self.hold_identity(Some(identity));
        }
        self
    }

    /// Hold `identity` (or forget the one held) and move the Host guard with it.
    /// Answers with the identity it replaced. Holding one marks it current;
    /// forgetting marks nothing current.
    fn hold_identity(&self, identity: Option<TailscaleIdentity>) -> Option<TailscaleIdentity> {
        self.identity_confirmed
            .store(identity.is_some(), Ordering::SeqCst);
        let admitted: Vec<String> = identity
            .as_ref()
            .and_then(admitted_own_name)
            .into_iter()
            .collect();
        let previous = {
            let mut slot = self
                .identity
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::replace(&mut *slot, identity)
        };
        self.own_name.replace(&admitted);
        previous
    }

    /// A look at the identity failed after one had succeeded: stop admitting the
    /// name until a look succeeds again. The identity stays held for the URLs on
    /// screen; only the Host guard's answer, which must not rest on a stale
    /// Funnel reading, is withdrawn.
    fn identity_lost(&self) {
        self.identity_confirmed.store(false, Ordering::SeqCst);
        self.own_name.replace(&[]);
        dux_core::logger::debug(
            "[server] could not read this machine's Tailscale name and Funnel state; dux stops \
             answering to the name until the next look succeeds.",
        );
    }

    /// Adopt an identity the watcher (or a one-shot look) read, and say what
    /// changed about it: a rename, a serve route arriving or leaving, Funnel.
    fn apply_identity(
        &mut self,
        identity: TailscaleIdentity,
        console: &Console,
        status: &LegStatus,
    ) {
        self.first_look_landed = true;
        let previous = self.hold_identity(Some(identity.clone()));
        self.settle_lockout(
            crate::serve_legs::lockout_after_look(&identity),
            crate::serve_legs::Because::Look,
            console,
            status,
        );
        for (tone, message) in identity_news(previous.as_ref(), &identity) {
            say(tone, &message, console, status);
        }
        self.say_serve_hint_once(console);
    }

    /// A look at Tailscale failed. The name is withdrawn until a look succeeds,
    /// and the reason decides the lockout: no CLI or no daemon means nothing
    /// can be published through Funnel, so dux serves; a CLI that fails leaves
    /// it unknown, which refuses on a serve that never got an answer and keeps
    /// whatever an earlier answer decided (a Funnel stays refused).
    fn look_failed(&mut self, reason: TailscaleUnavailable, console: &Console, status: &LegStatus) {
        let first_look = !std::mem::replace(&mut self.first_look_landed, true);
        if self.identity_confirmed.load(Ordering::SeqCst) {
            self.identity_lost();
        }
        let next = crate::serve_legs::lockout_after_failure(self.funnel_lockout.get(), &reason);
        self.settle_lockout(
            next,
            crate::serve_legs::Because::NoTailscaleHere,
            console,
            status,
        );
        // Inside a container, "no Tailscale here" says nothing about a
        // Tailscale outside it. dux serves anyway (that setup is the
        // operator's), and says so once, at start.
        if first_look
            && self.in_container
            && matches!(
                reason,
                TailscaleUnavailable::CommandMissing | TailscaleUnavailable::DaemonStopped
            )
        {
            say(
                dux_core::statusline::StatusTone::Warning,
                crate::serve_legs::CONTAINER_WARNING,
                console,
                status,
            );
        }
    }

    /// Adopt the outcome of a look the serve path ran itself (`dux server`'s
    /// startup look), exactly as a watcher's would be.
    pub(crate) fn apply_look(
        &mut self,
        look: Result<TailscaleIdentity, TailscaleUnavailable>,
        console: &Console,
        status: &LegStatus,
    ) {
        match look {
            Ok(identity) => self.apply_identity(identity, console, status),
            Err(reason) => self.look_failed(reason, console, status),
        }
    }

    /// Move the Funnel lockout and say so when that is news.
    fn settle_lockout(
        &self,
        next: crate::host_guard::FunnelLockout,
        because: crate::serve_legs::Because,
        console: &Console,
        status: &LegStatus,
    ) {
        let before = self.funnel_lockout.set(next);
        if let Some((tone, message)) = crate::serve_legs::lockout_news(before, next, because) {
            say(tone, &message, console, status);
        }
    }

    /// Say once, at start, that this serve does not consult Tailscale and what
    /// that costs. Both serves call it right after building their loop; a live
    /// switch to `no` says its own sentence.
    pub(crate) fn say_not_checking(&self, console: &Console) {
        if self.forced_no || !self.mode.wants_tailscale() {
            let message = crate::serve_legs::not_checking_tailscale(self.forced_no);
            dux_core::logger::warn(&format!("[server] {message}"));
            console.bind_degraded(&message);
        }
    }

    /// Say, once per run and on the console only, that `tailscale serve` would
    /// give dux an HTTPS address, when the identity says it would help. dux never
    /// runs that command itself.
    pub(crate) fn say_serve_hint_once(&mut self, console: &Console) {
        if self.serve_hint_given {
            return;
        }
        let Some(port) = self.primary.map(|primary| primary.port()) else {
            return;
        };
        let hint = self
            .identity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|identity| serve_hint(identity, port));
        if let Some(hint) = hint {
            self.serve_hint_given = true;
            dux_core::logger::info(&format!("[server] {hint}"));
            console.leg_changed(&hint);
        }
    }

    /// Hand the surfaces the serve's URL list when it changed (the flip's header
    /// reads it), and show the QR codes again when the URLs they carry changed:
    /// on the loop's
    /// first turn, when the leg binds or drops, and when the name or a serve
    /// route changes. The pair is the Tailscale IP URL and the MagicDNS one (the
    /// HTTPS serve URL when there is one). `legs` is `None` when the leg
    /// registry could not be read, which shows nothing new rather than a guess.
    ///
    /// Nothing is said while nothing has ever been shown: an empty pair only
    /// matters to a surface that is still showing an older one.
    pub(crate) fn refresh_surfaces(&mut self, legs: Option<Vec<SocketAddr>>, console: &Console) {
        let Some(legs) = legs else {
            return;
        };
        let identity = self.held_identity();
        let urls = tailnet_urls(
            &legs,
            identity.as_ref(),
            self.host_literals.load(Ordering::SeqCst),
        );
        let listed: Vec<String> = legs
            .iter()
            .map(|addr| format!("http://{addr}"))
            .chain(urls.extra())
            .collect();
        if self.last_urls.as_ref() != Some(&listed) {
            console.serve_urls(&listed);
            self.last_urls = Some(listed);
        }
        // The addresses the banner could not name, because they were not known
        // yet when it printed: listed again whenever they change, in the
        // banner's own row style, the same on both surfaces.
        let rows: Vec<ListenerRow> = urls
            .name
            .iter()
            .map(|url| ListenerRow {
                label: "Tailscale (MagicDNS)".to_string(),
                url: url.clone(),
            })
            .chain(urls.serve.iter().map(|url| ListenerRow {
                label: "Tailscale (HTTPS, tailscale serve)".to_string(),
                url: url.clone(),
            }))
            .collect();
        if self.last_rows != rows {
            if !rows.is_empty() {
                console.tailnet_rows(&rows);
            }
            self.last_rows = rows;
        }
        // No codes before the first look lands. A serve that cannot wait for it
        // (the flip, the background server) turns here first with the IP alone,
        // and printing that code means a second set a moment later when the
        // name arrives; every request answers "checking" until then anyway.
        if self.funnel_lockout.get() == crate::host_guard::FunnelLockout::Checking {
            return;
        }
        let magic_dns = urls.magic_dns().map(str::to_string);
        let pair: Vec<String> = urls.ip.into_iter().chain(magic_dns).collect();
        if self.last_qr.as_ref() == Some(&pair) {
            return;
        }
        let shown_before = self.last_qr.as_ref().is_some_and(|last| !last.is_empty());
        if !pair.is_empty() || shown_before {
            console.qr_codes(&pair);
        }
        self.last_qr = Some(pair);
    }

    fn held_identity(&self) -> Option<TailscaleIdentity> {
        self.identity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The command lane the serve loop listens on. Taken once.
    pub(crate) fn take_leg_receiver(&mut self) -> tokio::sync::mpsc::Receiver<(u64, WatchEvent)> {
        self.leg_rx.take().expect("the leg receiver is taken once")
    }

    /// Start the startup watcher on every mode but `no`. It PARKS before its
    /// first address look (the startup bind answered that a moment ago), but
    /// looks up a name it does not have yet at once. On `yes` it watches the
    /// name alone: the address is looked up once, but a Funnel switched on later
    /// must still withdraw the name.
    pub(crate) fn start_watcher_if_wanted(&mut self) {
        match self.mode {
            TailscaleMode::Auto => {
                self.start_watcher(false, true);
            }
            TailscaleMode::Yes => {
                self.start_watcher(false, false);
            }
            TailscaleMode::No => {}
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
    fn start_watcher(&mut self, probe_now: bool, addresses: bool) -> bool {
        self.stop_watcher();
        // Without a primary there is no port to hang the Tailscale leg on, but
        // the Funnel check still has to run, so such a serve watches the name
        // alone. The address it is handed is never bound.
        let (primary, addresses) = match self.primary {
            Some(primary) => (primary, addresses),
            None => {
                if addresses {
                    dux_core::logger::warn(
                        "[server] not watching for the Tailscale address: the address of the \
                         primary listener could not be read, so there is no port to serve the \
                         Tailscale leg on. dux still checks Tailscale for a Funnel.",
                    );
                }
                (SocketAddr::from(([127, 0, 0, 1], 0)), false)
            }
        };
        #[cfg(test)]
        if self.refuse_watcher_threads {
            self.watcher_failed_to_start();
            return false;
        }
        let generation = self.generation;
        let stop = Arc::new(AtomicBool::new(false));
        let watcher_stop = Arc::clone(&stop);
        let bound = Arc::clone(&self.bound);
        let tx = self.leg_tx.clone();
        let detect = Arc::clone(&self.detect);
        let identify = Arc::clone(&self.identify);
        let identity = Arc::clone(&self.identity);
        let confirmed = Arc::clone(&self.identity_confirmed);
        let period = self.watch_period;
        let started = std::thread::Builder::new()
            .name("dux-tailscale-watch".to_string())
            .spawn(move || {
                watch_tailscale(&Watch {
                    primary,
                    period,
                    probe_first: probe_now,
                    addresses,
                    detect: &*detect,
                    identify: &*identify,
                    bound: &|| bound.lock().ok().and_then(|slot| *slot),
                    // An identity that is not current counts as unknown, so the
                    // next successful look is sent and re-admits the name.
                    known_identity: &|| {
                        if !confirmed.load(Ordering::SeqCst) {
                            return None;
                        }
                        identity
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone()
                    },
                    emit: &|event| tx.blocking_send((generation, event)).is_ok(),
                    stop: &|| watcher_stop.load(Ordering::SeqCst),
                });
            })
            .is_ok();
        if started {
            self.watcher_stop = Some(stop);
            // Only an ADDRESS watcher binds the leg again, which is what this
            // flag tells a dying leg it may promise.
            self.watched.store(addresses, Ordering::SeqCst);
        } else {
            self.watcher_failed_to_start();
        }
        started
    }

    /// A thread dux cannot start is not a reason to refuse to serve; it is a
    /// reason to say the Tailscale leg is now static for this run, and to stop
    /// answering to the MagicDNS name, whose Funnel state nothing would watch.
    fn watcher_failed_to_start(&mut self) {
        self.identity_lost();
        // Nothing will look again, so the Funnel state cannot be known until a
        // restart: refuse, whatever an earlier look said (a Funnel stays one).
        if self.funnel_lockout.get() != crate::host_guard::FunnelLockout::Funnel {
            self.funnel_lockout
                .set(crate::host_guard::FunnelLockout::Unconfirmed);
        }
        dux_core::logger::warn(
            "[server] could not start the Tailscale watcher. dux is serving on the addresses it \
             bound at startup, and stops answering to this machine's MagicDNS name because \
             nothing would notice a Tailscale Funnel being switched on; restart dux to watch \
             again.",
        );
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
    pub(crate) fn leg_sender(&self) -> tokio::sync::mpsc::Sender<(u64, WatchEvent)> {
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
#[allow(clippy::too_many_arguments)]
async fn run_serve_loop(
    mut tasks: tokio::task::JoinSet<()>,
    shutdown: ServeShutdown,
    mut commands: tokio::sync::mpsc::Receiver<(u64, WatchEvent)>,
    mut mode_requests: tokio::sync::mpsc::Receiver<crate::serve_legs::ModeRequest>,
    app: Router,
    console: Console,
    status: LegStatus,
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
        // Every arm below can move a URL the QR codes carry (a leg bound or
        // dropped, a name read, a mode changed), so the codes are checked once
        // per turn rather than at each of those places.
        ts.refresh_surfaces(shutdown.leg_addrs(), &console);
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
                            &status,
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
                    detected.unwrap_or((
                        Err(TailscaleUnavailable::CommandFailed),
                        Err(TailscaleUnavailable::CommandFailed),
                    )),
                    &mut ts,
                    &mut tasks,
                    &shutdown,
                    &app,
                    &console,
                    &status,
                    &mut last_bind_failure,
                )
                .await;
            }
            Some(joined) = tasks.join_next(), if !tasks.is_empty() => {
                finish_leg_task(joined, &shutdown, &ts.bound, &mut last_bind_failure);
            }
            // The dwell a leg transition is held for. The status is the only
            // thing delayed: the console line and the log went out at once. A
            // sleep rather than a tick, so a loop with nothing held waits on
            // nothing.
            due = async {
                let due = status
                    .pending_deadline()
                    .expect("guarded by the arm's condition");
                tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
                due
            }, if status.pending_deadline().is_some() => {
                // The deadline itself, not the clock: a timer that fires a
                // fraction early would otherwise leave the sentence held and
                // this arm re-arming on a deadline already past.
                status.flush_due(due);
            }
            command = commands.recv() => {
                apply_current_generation_command(
                    command,
                    &mut ts,
                    &mut tasks,
                    &shutdown,
                    &app,
                    &console,
                    &status,
                    &mut last_bind_failure,
                )
                .await;
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

/// Act on a watcher's leg command, unless it belongs to a watcher generation the
/// loop has already left behind.
///
/// A watcher parked in a five-second probe when the mode changed comes back with a
/// command for the mode dux already left; acting on it would re-bind the leg the
/// change just let go. `None` cannot arrive while the loop runs: the loop holds a
/// sender for its whole life, so a mode that runs no watcher simply has nobody
/// sending.
#[allow(clippy::too_many_arguments)]
async fn apply_current_generation_command(
    command: Option<(u64, WatchEvent)>,
    ts: &mut TailscaleLoop,
    tasks: &mut tokio::task::JoinSet<()>,
    shutdown: &ServeShutdown,
    app: &Router,
    console: &Console,
    status: &LegStatus,
    last_bind_failure: &mut Option<SocketAddr>,
) {
    let Some((generation, event)) = command else {
        return;
    };
    if generation != ts.generation {
        return;
    }
    let command = match event {
        WatchEvent::Leg(command) => command,
        WatchEvent::IdentityFailed(reason) => {
            ts.look_failed(reason, console, status);
            return;
        }
        WatchEvent::Identity(identity) => {
            ts.apply_identity(identity, console, status);
            return;
        }
    };
    apply_leg_command(
        command,
        tasks,
        shutdown,
        app,
        console,
        status,
        &ts.bound,
        last_bind_failure,
    )
    .await;
}

/// Account for one serve leg's task having ended.
///
/// A task that PANICKED recorded nothing, so record it here and trip the lane: a
/// panicking serve task is not a leg going quietly, it is a bug, and the server
/// must not limp on half-dead, so the other listeners are shut down too.
///
/// Either way the leg may have been the Tailscale one dying on its own (its accept
/// loop failed mid-run, or it panicked): it forgot itself from the registry but
/// nothing cleared the watcher-facing "what is bound" cell, and a watcher that
/// still believes the leg is bound plans Nothing forever. Reconciling here keeps
/// the serve loop the cell's ONE writer, rather than letting a dying task write it
/// from underneath.
fn finish_leg_task(
    joined: Result<(), tokio::task::JoinError>,
    shutdown: &ServeShutdown,
    bound_tailscale: &Arc<std::sync::Mutex<Option<SocketAddr>>>,
    last_bind_failure: &mut Option<SocketAddr>,
) {
    if let Err(join_err) = joined {
        dux_core::logger::error(&format!(
            "[server] a serve task panicked: {join_err}. Shutting the other \
             listeners down so the server does not limp on half-dead."
        ));
        shutdown.record_failure(anyhow::anyhow!("a serve task panicked: {join_err}"));
    }
    reconcile_bound_tailscale(shutdown, bound_tailscale, last_bind_failure);
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
    status: &LegStatus,
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
    // Before the first step, not part way through one: without a primary there
    // is no port to hang a leg on, so a mode that wants one must not open the
    // Host guard's tailnet-literal rule or move the recorded mode either. A
    // refusal the run cannot lift outranks it.
    if !matches!(steps.first(), Some(ModeStep::Refuse))
        && mode.wants_tailscale()
        && ts.primary.is_none()
    {
        let _ = reply.send(TailscaleModeOutcome::NoPrimary);
        return;
    }
    let mut detached = false;
    for step in steps {
        match step {
            ModeStep::Refuse => {
                let _ = reply.send(TailscaleModeOutcome::RefusedForcedNo);
                return;
            }
            ModeStep::StopWatcher => ts.stop_watcher(),
            ModeStep::SetHostLiterals(allowed) => {
                // Coming back from `no`, nothing is known about Funnel yet, so
                // nothing is served until this mode's first look lands.
                if allowed && matches!(ts.mode, TailscaleMode::No) {
                    ts.funnel_lockout
                        .set(crate::host_guard::FunnelLockout::Checking);
                }
                ts.host_literals.store(allowed, Ordering::SeqCst);
            }
            ModeStep::ForgetIdentity => {
                ts.hold_identity(None);
                // `no` is the mode in which dux does not consult Tailscale at
                // all, so it lifts any refusal: an explicit choice, said loudly.
                let before = ts
                    .funnel_lockout
                    .set(crate::host_guard::FunnelLockout::Open);
                if before != crate::host_guard::FunnelLockout::Open {
                    say(
                        dux_core::statusline::StatusTone::Warning,
                        &crate::serve_legs::lockout_lifted_by_no(),
                        console,
                        status,
                    );
                }
            }
            ModeStep::Unbind(addr) => {
                apply_leg_command(
                    LegCommand::Unbind(addr),
                    tasks,
                    shutdown,
                    app,
                    console,
                    status,
                    &ts.bound,
                    last_bind_failure,
                )
                .await;
                detached = true;
            }
            ModeStep::StartWatcher { probe_now } => {
                ts.mode = mode;
                // The primary is there, so the only failure left is a thread
                // that would not start, which the log names; either way the
                // answer the caller needs is that nothing is watching.
                if !ts.start_watcher(probe_now, true) {
                    let _ = reply.send(TailscaleModeOutcome::NoPrimary);
                    return;
                }
            }
            ModeStep::DetectAndBind => {
                ts.mode = mode;
                // A generation of its own, so a watcher this change replaced
                // cannot land a command against the answer of this probe.
                ts.generation += 1;
                let detect = Arc::clone(&ts.detect);
                let identify = Arc::clone(&ts.identify);
                *pending_detect = Some(PendingDetect {
                    generation: ts.generation,
                    reply,
                    // `spawn_blocking`, never the loop: the probes are bounded
                    // but blocking subprocess calls, and awaiting them inline
                    // would stop the legs and the parent lane for their window.
                    task: tokio::task::spawn_blocking(move || (detect(), identify())),
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
    (detected, identified): OneShotLook,
    ts: &mut TailscaleLoop,
    tasks: &mut tokio::task::JoinSet<()>,
    shutdown: &ServeShutdown,
    app: &Router,
    console: &Console,
    status: &LegStatus,
    last_bind_failure: &mut Option<SocketAddr>,
) {
    let PendingDetect {
        generation,
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
    // The name is read whatever became of the address: `yes` is the mode's one
    // look, and a name that answered is worth holding either way.
    match identified {
        Ok(identity) => ts.apply_identity(identity, console, status),
        // A failed look is handled exactly as a watcher's: the name is
        // withdrawn and the reason decides the lockout.
        Err(reason) => ts.look_failed(reason, console, status),
    }
    // `yes` looked for the address once, just now, and never looks again; the
    // name it keeps watching, so a Funnel switched on later is noticed. The
    // watcher's first look waits a period, because this look just happened.
    if matches!(ts.mode, TailscaleMode::Yes) {
        ts.start_watcher(false, false);
    }
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
            status,
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
        _ => TailscaleModeOutcome::Applied { bound },
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
#[allow(clippy::too_many_arguments)]
async fn apply_leg_command(
    command: LegCommand,
    tasks: &mut tokio::task::JoinSet<()>,
    shutdown: &ServeShutdown,
    app: &Router,
    console: &Console,
    status: &LegStatus,
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
                    status.clone(),
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
                status.changed(&message);
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
                    status.degraded(&warning);
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
                status.changed(&message);
            }
        }
    }
}

/// Say one Tailscale sentence everywhere it belongs: the log, the console (the
/// `dux server` terminal or the flip's activity panel), and both surfaces.
fn say(
    tone: dux_core::statusline::StatusTone,
    message: &str,
    console: &Console,
    status: &LegStatus,
) {
    match tone {
        dux_core::statusline::StatusTone::Warning => {
            dux_core::logger::warn(&format!("[server] {message}"));
            console.bind_degraded(message);
        }
        _ => {
            dux_core::logger::info(&format!("[server] {message}"));
            console.leg_changed(message);
        }
    }
    status.identity_news(tone, message);
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
        .with_pty_send_timeout_seconds(
            dux_core::config_effective::effective_pty_send_timeout_seconds(
                server.pty_send_timeout_seconds,
            ),
        )
        .with_heartbeat_deadline_seconds(
            dux_core::config_effective::effective_heartbeat_deadline_seconds(
                server.heartbeat_deadline_seconds,
                server.heartbeat_seconds,
            ),
        )
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
    /// Install the tokio SIGINT/SIGTERM handler and trip `flag` when one
    /// arrives. For a serve that is the only thing running: the flip, whose
    /// status screen has taken the terminal over. `restore_terminal` gives the
    /// terminal back before a second signal forces the process out, because
    /// that exit runs no destructor and would leave the status screen's raw
    /// mode and alternate screen behind.
    Adopt {
        flag: Arc<AtomicBool>,
        restore_terminal: Option<RestoreTerminal>,
        quit: Arc<QuitForce>,
    },
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

        // The shared shutdown primitive: the SAME [`ServeShutdown`] the CLI serve
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
                    .with_live_own_magicdns_name(mode_control.own_magicdns_name())
                    .with_live_funnel_lockout(mode_control.funnel_lockout())
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
            // Not read here: this runs on the terminal UI's thread, which must
            // not wait on a subprocess. The watcher looks it up before its first
            // park, and on `yes` a one-shot look runs off this thread.
            // Every leg shares one port; the configured one is the last resort
            // for a serve whose listener addresses could not be read, so the
            // Funnel check runs whatever happened to the primary.
            let identity_port = primary
                .or(tailscale_leg)
                .map(|addr| addr.port())
                .unwrap_or(config.server.port);
            tailscale_loop = tailscale_loop.with_identity(
                Arc::new(move || dux_core::tailscale::detect_identity(identity_port)),
                None,
            );
            tailscale_loop.start_watcher_if_wanted();
            tailscale_loop.say_not_checking(&console);
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
            // Cloned before `handle` is dropped below: the leg's news is owed to
            // the terminal UI beside this serve as much as to the browsers.
            let leg_status = LegStatus::new(handle.clone());
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
                    leg_status.clone(),
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
                leg_status,
                tailscale_loop,
            ))
        };
        // Kept for the SIGUSR1 reload task, which ends with the serve and so
        // never holds the request side open past it.
        let reload_handle = handle.clone();
        // The router holds its own cloned handle(s); drop ours so only the serve
        // tasks keep the request side alive.
        drop(handle);

        let installed_signal_handlers = match signals {
            SignalPolicy::Adopt {
                flag,
                restore_terminal,
                quit,
            } => {
                let force_exit = ForceExit::new(console.clone(), restore_terminal, quit);
                runtime.spawn(async move {
                    shutdown_signal(force_exit).await;
                    flag.store(true, Ordering::SeqCst);
                });
                // The flip owns the engine, so SIGUSR1 runs the engine actor's
                // reload here; the terminal UI's loop is not running to do it.
                runtime.spawn(reload_signal::reload_on_signal(
                    reload_handle,
                    shutdown.subscribe(),
                ));
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

    /// The addresses this serve is reachable on RIGHT NOW, read from the live leg
    /// registry rather than remembered from the bind. `None` means the registry
    /// could not be read at all, never that nothing is being served.
    ///
    /// The Tailscale leg comes and goes under a running serve, so a list captured
    /// at start goes stale the first time the interface moves.
    ///
    /// This machine's MagicDNS URL and its `tailscale serve` routes follow the
    /// listener addresses, read from what the Tailscale watcher last saw, so the
    /// first entry is always a listener (the loopback one) and a rename or a new
    /// serve route shows up here with no restart.
    pub(crate) fn live_urls(&self) -> Option<Vec<String>> {
        let legs = self.shutdown.leg_addrs()?;
        let extra = self.tailscale_mode.tailnet_urls(&legs).extra();
        Some(
            legs.into_iter()
                .map(|addr| format!("http://{addr}"))
                .chain(extra)
                .collect(),
        )
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
///   ReturnToTui)` with PTYs UNTOUCHED, so the TUI resumes around the same agents.
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
///
/// The console log is the same one `dux server` prints: `startup` carries what
/// the terminal UI's pre-flight learned (its warnings, the Tailscale bind
/// failures, whether an address was detected), and those lines plus the banner
/// open the log before anything is served. `hooks` are what the status screen
/// lends when it is on screen: the terminal restore a forced exit runs.
/// `on_shutdown_wait` gets a turn on this thread on every pass of the quit's
/// wait for the children: the status screen reads its keys and redraws there,
/// and answers true when a second Ctrl-c key asks to stop waiting (the screen
/// holds the terminal in raw mode, so Ctrl-c is a key, not a signal).
#[allow(clippy::too_many_arguments)]
pub fn serve_with_engine(
    mut engine: Engine,
    listeners: Vec<std::net::TcpListener>,
    activity: dux_core::activity::ActivityRing,
    startup: StartupNotes,
    hooks: FlipHooks,
    mut on_tick: impl FnMut() -> ServerTick,
    mut on_shutdown_status: impl FnMut(&str),
    mut on_shutdown_wait: impl FnMut() -> bool,
) -> Result<(Engine, ServerExit)> {
    warn_if_ui_not_built();
    // The flip owns the terminal with its themed status screen, so this console
    // writes NOTHING to stdout; it records every line into the shared ring the
    // status screen's log viewer draws, the same lines `dux server` prints.
    let console = Console::capture(activity);
    // The QR codes reach the log viewer as lines, exactly as `dux server`
    // prints them.
    console.set_qr_codes(engine.config.server.qr_codes);
    for warning in &startup.warnings {
        console.warn(warning);
    }
    // Both in-app serves are LOCAL MODE: the loopback leg is the required one
    // and anything else is the best-effort Tailscale leg.
    let legs: Vec<(SocketAddr, bool)> = listeners
        .iter()
        .filter_map(|l| l.local_addr().ok())
        .map(|addr| (addr, addr.ip().is_loopback()))
        .collect();
    console.banner(&serve_banner(
        dux_core::display_version(),
        &legs,
        &startup.bind_warnings,
        engine.config.server.tailscale_mode(),
        startup.tailscale_detected,
    ));
    let (handle, ends) = engine_actor::build_actor_channels(&engine);
    engine_actor::spawn_global_workers(&mut engine);

    // Grab the teardown flag before the handle moves into the router. `ServeCore`
    // trips it the instant serving ends (before axum graceful shutdown) so any
    // PTY forwarders parked on their blocking `recv_timeout` exit within one poll
    // window, even on ReturnToTui, where the engine and its PtyClient senders
    // stay alive and the forwarders' channels would otherwise never disconnect.
    let shutdown_flag = handle.shutdown_flag();

    // Set by the signal task; polled by the control closure so a SIGINT/SIGTERM
    // received while serving breaks the engine loop too (not just axum). Distinct
    // from the serve's failure flag because a signal means QuitProcess, a failure
    // means ReturnToTui-with-error.
    let signal_quit = Arc::new(AtomicBool::new(false));

    // The flip has taken the terminal over with its own status screen, so it OWNS
    // the process's stop signals for as long as it serves.
    let access_log = engine.config.server.access_log;
    let FlipHooks { restore_terminal } = hooks;
    // Shared by the second-signal hatch and the quit's wait, so either way of
    // forcing the quit kills the children and logs the same lines.
    let quit = Arc::new(QuitForce::default());
    let mut core = ServeCore::start(
        handle,
        listeners,
        &engine.config,
        console.clone(),
        // `[server] access_log` governs the flip exactly as it does `dux server`:
        // the user decided (2026-10-02) that the two logs match completely, so
        // the viewer carries the access log too.
        access_log,
        SignalPolicy::Adopt {
            flag: Arc::clone(&signal_quit),
            restore_terminal,
            quit: Arc::clone(&quit),
        },
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
        // Silent here because the loop never runs the flip's shutdown: the
        // flip runs it itself below, after the loop exits, and prints its
        // progress through its own console (the viewer's log, the same lines
        // `dux server` prints) and the status callback.
        engine_actor::ShutdownEcho::Silent,
        engine_actor::ServeSurface::Flip,
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
        let grace = dux_core::config::shutdown_grace(engine.config.server.shutdown_timeout_seconds);
        // The one wind-down `dux server` runs too, so the viewer carries the
        // lines `dux server` prints; the status callback gets them as well,
        // and the status screen gets a turn on every pass of the wait.
        wind_down_children(
            &mut engine,
            &console,
            grace,
            &quit,
            &mut on_shutdown_status,
            || {
                if on_shutdown_wait() {
                    quit.request(&console);
                }
            },
        );
        if quit.is_requested() {
            exit = ServerExit::ForceQuit(ForceQuitHandle(Arc::clone(&quit)));
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
    let exit = flip_outcome(exit, serve_error)?;
    Ok((engine, exit))
}

/// How the flip ends, given how its loop exited and whether the serve failed. A
/// serve failure is reported rather than a clean exit, except under a forced
/// quit: the user asked to be out now, so the quit wins and exits 130 with the
/// force line, and the failure goes to `dux.log`.
fn flip_outcome(exit: ServerExit, serve_error: Option<anyhow::Error>) -> Result<ServerExit> {
    match (exit, serve_error) {
        (ServerExit::ForceQuit(handle), Some(err)) => {
            dux_core::logger::error(&format!(
                "[server] the web server also failed while being forced to quit: {err:#}"
            ));
            Ok(ServerExit::ForceQuit(handle))
        }
        (_, Some(err)) => Err(err),
        (exit, None) => Ok(exit),
    }
}

/// Wind the children down at the end of a quit, the one way both modes do it:
/// SIGTERM, then wait out the grace unless a forced quit cuts the wait short,
/// then kill what is left. Logs the start, and the result as a warning when
/// something had to be force-closed, through `console` and `status`; a forced
/// quit's own line lands between them, at the moment it was asked for. Marks
/// `quit` started, so a second signal knows there is an owner to hand the exit
/// to. `on_wait` gets a turn on every pass of the wait. `None` when there was
/// nothing to wind down.
pub(crate) fn wind_down_children(
    engine: &mut Engine,
    console: &Console,
    grace: Duration,
    quit: &QuitForce,
    mut status: impl FnMut(&str),
    on_wait: impl FnMut(),
) -> Option<dux_core::engine::ShutdownReport> {
    let agents = engine.providers.len();
    let terminals = engine.companion_terminals.len();
    if agents + terminals == 0 {
        return None;
    }
    quit.start();
    let start = dux_core::engine::format_shutdown_start(agents, terminals, grace);
    console.progress(&start);
    status(&start);
    let report = engine.shutdown_ptys_waiting(grace, Some(&quit.requested), on_wait);
    let result = dux_core::engine::format_shutdown_result(&report);
    if report.timed_out {
        console.warn(&result);
    } else {
        console.progress(&result);
    }
    status(&result);
    Some(report)
}

/// A quit being forced: asked for by a second Ctrl-c (a signal anywhere, a key
/// in the flip), and ended by exactly one thread.
#[derive(Default)]
pub(crate) struct QuitForce {
    requested: AtomicBool,
    /// The wind-down is under way: only then is a forced exit worth a (bounded)
    /// wait for the children to be killed.
    started: AtomicBool,
    /// Taken by whichever thread ends the process for a forced quit (the
    /// engine's owner, or the signal hatch when the owner is late), so exactly
    /// one calls exit.
    exit_claimed: AtomicBool,
}

impl QuitForce {
    /// Ask for the quit to be forced. The first ask logs the force line, so it
    /// is logged once however many ways it was asked; later asks log nothing.
    /// Whether this ask was the first.
    pub(crate) fn request(&self, console: &Console) -> bool {
        if self
            .requested
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        dux_core::logger::error(&format!("[server] {FORCE_EXIT_MESSAGE}"));
        console.error(FORCE_EXIT_MESSAGE);
        true
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    pub(crate) fn start(&self) {
        self.started.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// Take the right to end the process. True for exactly one caller.
    pub(crate) fn claim_exit(&self) -> bool {
        self.exit_claimed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    fn is_exit_claimed(&self) -> bool {
        self.exit_claimed.load(Ordering::SeqCst)
    }

    /// Wait, at most `bound`, for the engine's owner to take the exit.
    fn wait_for_owner(&self, bound: Duration) {
        let deadline = std::time::Instant::now() + bound;
        while !self.is_exit_claimed() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// End the process after the flip's forced quit: drop the engine first, so the
/// config writes still queued land on disk (the queue flushes, bounded, as it
/// drops), then say why on the shell's screen and exit with 130. `exit` is
/// injected so a test can watch it without ending the test process.
///
/// When the signal hatch already took the exit (this thread was late), the
/// engine is still dropped but exit is not called: the hatch is exiting, and the
/// caller must not end the process a second time.
pub fn finish_forced_quit(engine: Engine, handle: &ForceQuitHandle, exit: impl FnOnce(i32)) {
    drop(engine);
    if handle.0.claim_exit() {
        print_force_message_once();
        exit(130);
    }
}

/// Prints the force line to stderr once per process, whichever of the flip's
/// two exits (the hatch's, or the binary's after a forced quit) gets there
/// first.
pub fn print_force_message_once() {
    static PRINTED: AtomicBool = AtomicBool::new(false);
    if !PRINTED.swap(true, Ordering::SeqCst) {
        eprintln!("{FORCE_EXIT_MESSAGE}");
    }
}

/// How long a forced exit waits for the children to be killed and the result
/// logged before it exits anyway. A safety bound on an exit path, not a
/// preference: the hatch has to exit even when the wind-down never started.
const FORCE_SETTLE_BOUND: Duration = Duration::from_secs(3);

/// Resolves when the process receives SIGINT (Ctrl-C) or SIGTERM. The first such
/// signal resolves this future (the caller then triggers a graceful shutdown)
/// and also arms a watcher so a SECOND signal forces an immediate exit, rather
/// than leaving the operator trapped if the graceful drain wedges.
async fn shutdown_signal(force_exit: ForceExit) {
    // Install both handlers ONCE up front and reuse the same streams for the
    // first wait AND the second-signal force-quit watcher. Re-subscribing fresh
    // after the first signal fired would race: a rapid second signal could arrive
    // in the window before a newly-created listener is registered and be missed.
    // A persistent `Signal` stream stays armed and catches the next delivery
    // whenever it is next polled.
    let mut interrupt = install_signal(
        tokio::signal::unix::SignalKind::interrupt(),
        "SIGINT (Ctrl-C)",
        &force_exit.console,
    );
    let mut terminate = install_signal(
        tokio::signal::unix::SignalKind::terminate(),
        "SIGTERM",
        &force_exit.console,
    );

    if interrupt.is_none() && terminate.is_none() {
        // Neither handler installed, so we can observe no stop signal. Park so this
        // future never resolves spuriously; `install_signal` already logged loudly.
        std::future::pending::<()>().await;
    }

    next_terminate_signal(&mut interrupt, &mut terminate).await;

    // A graceful shutdown has now been requested. If it wedges (a stuck PTY
    // write, a client socket that never closes, an unbounded connection drain), a
    // SECOND Ctrl-C/SIGTERM must NOT be swallowed, or the operator is trapped and
    // forced to `kill -9`. Reuse the already-armed streams (so there is no
    // re-registration gap) and force-exit on the next signal. This deliberately
    // bypasses the (possibly stuck) graceful path: the "I really mean stop" escape
    // hatch, mirroring how most servers treat a second Ctrl-C. 130 = 128 + SIGINT,
    // the conventional interrupted-exit code.
    tokio::spawn(async move {
        next_terminate_signal(&mut interrupt, &mut terminate).await;
        fire_force_exit(force_exit, |code| std::process::exit(code));
    });
}

/// Run a forced exit on a thread of its own. Its wait for the engine's owner
/// sleeps, and parked on a runtime worker that sleep would hold up the owner's
/// own way out, which tears that runtime down (in the flip, bounded at 2 s) and
/// would then arrive late for the exit it was meant to take.
pub(crate) fn fire_force_exit(force_exit: ForceExit, exit: impl FnOnce(i32) + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("dux-force-exit".to_string())
        .spawn(move || force_exit.run(exit));
    if let Err(err) = spawned {
        // No thread to be had: exit straight away rather than not at all.
        dux_core::logger::error(&format!(
            "[server] could not start the forced-exit thread ({err}); exiting now"
        ));
        std::process::exit(130);
    }
}

/// Hands the terminal back to the shell: the flip's status screen supplies it,
/// `dux server` has nothing to restore.
pub type RestoreTerminal = Arc<dyn Fn() + Send + Sync>;

/// The line a second stop signal leaves on its way out.
pub const FORCE_EXIT_MESSAGE: &str = "second interrupt received during shutdown: stopping \
     whatever is still running, then exiting.";

/// What a second stop signal does: force the quit (the same line in both modes,
/// and the wait for the children cut short so they are killed), wait a bounded
/// moment for that to settle when a wind-down is under way (and not at all
/// before one has started), make sure the log is written, give the terminal back
/// when a status screen had it, and only then exit with 130 (128 + SIGINT).
///
/// The order is the point. `std::process::exit` runs no destructor, so the flip's
/// status screen would otherwise leave raw mode and the alternate screen behind,
/// and a message printed before the restore would land on the alternate screen
/// and vanish with it. After the restore it is printed to stderr, on the shell's
/// own screen, because the viewer that would have shown it is gone.
#[derive(Clone)]
pub(crate) struct ForceExit {
    console: Console,
    restore_terminal: Option<RestoreTerminal>,
    quit: Arc<QuitForce>,
    settle_bound: Duration,
}

impl ForceExit {
    pub(crate) fn new(
        console: Console,
        restore_terminal: Option<RestoreTerminal>,
        quit: Arc<QuitForce>,
    ) -> Self {
        Self {
            console,
            restore_terminal,
            quit,
            settle_bound: FORCE_SETTLE_BOUND,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_settle_bound(mut self, bound: Duration) -> Self {
        self.settle_bound = bound;
        self
    }

    /// Run the sequence, ending in `exit(130)`. `exit` is injected so a test can
    /// watch the order without ending the test process.
    pub(crate) fn run(&self, exit: impl FnOnce(i32)) {
        self.quit.request(&self.console);
        // While a wind-down runs, it is killing the children; its owner then
        // drops the engine (landing queued config writes) and exits itself.
        // Leave the exit to it, waiting only up to the bound. Before a
        // wind-down has started (with no agent or terminal running, one never
        // starts) there is nothing to wait for and the exit is immediate, so
        // queued config writes are not waited for then.
        if self.quit.is_started() {
            self.quit.wait_for_owner(self.settle_bound);
        }
        if !self.quit.claim_exit() {
            // The owner took it and is exiting: one exit, not two.
            return;
        }
        self.console.flush();
        if let Some(restore) = &self.restore_terminal {
            restore();
        }
        // The line is on stdout already when this console prints there (`dux
        // server`); a flip's console only fed a viewer that is gone, so it is
        // said on stderr, on the shell's own screen.
        if !self.console.is_active() {
            print_force_message_once();
        }
        exit(130);
    }
}

/// Install a SIGINT/SIGTERM handler, returning the stream, or `None` (logged
/// loudly) if registration fails, so the caller can still rely on the other
/// signal. `label` is the human name used in the failure message.
fn install_signal(
    kind: tokio::signal::unix::SignalKind,
    label: &str,
    console: &Console,
) -> Option<tokio::signal::unix::Signal> {
    match tokio::signal::unix::signal(kind) {
        Ok(sig) => Some(sig),
        Err(e) => {
            // Registering this handler failed: say so loudly instead of dropping
            // the error. The other signal still gives a graceful stop; if BOTH
            // fail, `shutdown_signal` parks rather than firing spuriously.
            let msg = format!(
                "[server] failed to install the {label} handler: {e}. {label} will not stop the \
                 server; rely on the other signal (Ctrl-C for SIGINT, systemctl/docker stop for \
                 SIGTERM)."
            );
            dux_core::logger::error(&msg);
            // On the console rather than raw stderr: in the flip, stderr is
            // the status screen's alternate screen.
            console.error(&msg);
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
                // teardown), which is NOT a delivered signal: resolving on it
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
    use dux_core::config::{DuxPaths, PlanAddr, TailscaleMode};
    use dux_core::engine::Command;

    #[tokio::test]
    async fn a_panicking_serve_task_trips_the_lane_while_a_clean_end_does_not() {
        let ts: std::net::SocketAddr = "100.64.0.5:8080".parse().unwrap();
        let shutdown = crate::serve_legs::ServeShutdown::for_watched(true);
        let _leg = shutdown.register_leg(ts);
        let cell = std::sync::Arc::new(std::sync::Mutex::new(Some(ts)));
        let mut streak = None;

        super::finish_leg_task(Ok(()), &shutdown, &cell, &mut streak);
        assert!(
            !shutdown.is_failed(),
            "a task ending cleanly is not a reason to shut the other listeners down"
        );

        let panicked = tokio::spawn(async { panic!("a serve task fell over") })
            .await
            .expect_err("the task panicked");
        super::finish_leg_task(Err(panicked), &shutdown, &cell, &mut streak);
        assert!(shutdown.is_failed(), "a panic trips the lane");
    }

    #[tokio::test]
    async fn a_leg_task_that_ends_clears_a_cell_the_registry_no_longer_backs() {
        let ts: std::net::SocketAddr = "100.64.0.5:8080".parse().unwrap();
        let shutdown = crate::serve_legs::ServeShutdown::for_watched(true);
        let cell = std::sync::Arc::new(std::sync::Mutex::new(Some(ts)));
        let mut streak = Some(ts);
        super::finish_leg_task(Ok(()), &shutdown, &cell, &mut streak);
        assert_eq!(
            *cell.lock().unwrap(),
            None,
            "an address no leg is registered for stops looking bound"
        );
        assert_eq!(streak, None);
    }

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

    /// The Tailscale leg leaving is news for whoever is looking at dux, not just
    /// for `dux server`'s own terminal: a browser on the tailnet is exactly the
    /// client that loses the address, and a terminal UI serving in the background
    /// writes its console nowhere at all.
    #[tokio::test]
    async fn a_leg_that_goes_away_reaches_the_surfaces_as_well_as_the_console() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let paths = dux_core::config::DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        let mut engine = crate::test_support::bootstrap_test_engine(&paths).expect("engine");
        let (handle, ends) = crate::engine_actor::build_actor_channels(&engine);
        let mut svc = crate::engine_actor::EngineService::new(
            &engine,
            ends,
            crate::engine_actor::ShutdownEcho::Silent,
        );

        let ts: std::net::SocketAddr = "100.64.0.5:8080".parse().unwrap();
        let shutdown = crate::serve_legs::ServeShutdown::for_watched(true);
        let _leg = shutdown.register_leg(ts);
        let cell = std::sync::Arc::new(std::sync::Mutex::new(Some(ts)));
        let mut streak = None;
        let mut tasks = tokio::task::JoinSet::new();

        let status_lane = crate::serve_legs::LegStatus::new(handle);
        super::apply_leg_command(
            crate::serve_legs::LegCommand::Unbind(ts),
            &mut tasks,
            &shutdown,
            &axum::Router::new(),
            &crate::console::Console::noop(),
            &status_lane,
            &cell,
            &mut streak,
        )
        .await;
        // The sentence is held for its dwell, so the serve loop's own clock is
        // what lets it out; here that clock is this line.
        status_lane.flush_due(std::time::Instant::now() + crate::serve_legs::LEG_SETTLE_DWELL);

        svc.drain_requests(&mut engine);
        let posted = engine.worker_rx.try_recv().expect("a status on the lane");
        let dux_core::worker::WorkerEvent::PollerStatus(status) = posted else {
            panic!("the leg's news rides the poller-status lane");
        };
        assert_eq!(
            status.key.as_deref(),
            Some(crate::serve_legs::TAILSCALE_LEG_KEY)
        );
        assert_eq!(status.tone, dux_core::statusline::StatusTone::Info);
        assert!(status.message.contains("went away"), "{}", status.message);
    }

    /// An interface that comes and goes inside one dwell is one story, not four.
    ///
    /// Every transition lands on the same key, and a toast re-raised on a fixed
    /// id restarts its window rather than expiring, so a sentence per bind
    /// pinned the flap's news open for as long as the flapping lasted.
    #[tokio::test]
    async fn a_flapping_leg_is_one_sentence_and_one_answer_when_it_settles() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let paths = dux_core::config::DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        let mut engine = crate::test_support::bootstrap_test_engine(&paths).expect("engine");
        let (handle, ends) = crate::engine_actor::build_actor_channels(&engine);
        let mut svc = crate::engine_actor::EngineService::new(
            &engine,
            ends,
            crate::engine_actor::ShutdownEcho::Silent,
        );

        // A real loopback address, so the bind half of the flap genuinely binds
        // and the unbind half genuinely stops a registered leg.
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let shutdown = crate::serve_legs::ServeShutdown::for_watched(true);
        let cell = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut streak = None;
        let mut tasks = tokio::task::JoinSet::new();
        let status_lane = crate::serve_legs::LegStatus::new(handle);

        for command in [
            crate::serve_legs::LegCommand::Bind(addr),
            crate::serve_legs::LegCommand::Unbind(addr),
            crate::serve_legs::LegCommand::Bind(addr),
            crate::serve_legs::LegCommand::Unbind(addr),
        ] {
            super::apply_leg_command(
                command,
                &mut tasks,
                &shutdown,
                &axum::Router::new(),
                &crate::console::Console::noop(),
                &status_lane,
                &cell,
                &mut streak,
            )
            .await;
        }

        svc.drain_requests(&mut engine);
        let first = drain_one_leg_status(&engine);
        assert_eq!(first.tone, dux_core::statusline::StatusTone::Warning);
        assert!(
            first.message.contains("going up and down"),
            "{}",
            first.message
        );
        assert!(
            engine.worker_rx.try_recv().is_err(),
            "four transitions inside one dwell are one sentence"
        );

        status_lane.flush_due(std::time::Instant::now() + crate::serve_legs::LEG_SETTLE_DWELL);
        svc.drain_requests(&mut engine);
        let settled = drain_one_leg_status(&engine);
        assert!(
            settled.message.contains("has settled"),
            "{}",
            settled.message
        );
        assert!(
            engine.worker_rx.try_recv().is_err(),
            "and the settling is one answer, on the same key"
        );
        assert_eq!(first.key, settled.key);
    }

    /// A bind that fails is the other half of the leg's news, and it is the half
    /// a watcher retries: the address is there, the listener will not start, and
    /// only a warning on the surfaces says why the tailnet cannot reach dux.
    #[tokio::test]
    async fn a_bind_that_fails_warns_the_surfaces_and_not_only_the_console() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let paths = dux_core::config::DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        let mut engine = crate::test_support::bootstrap_test_engine(&paths).expect("engine");
        let (handle, ends) = crate::engine_actor::build_actor_channels(&engine);
        let mut svc = crate::engine_actor::EngineService::new(
            &engine,
            ends,
            crate::engine_actor::ShutdownEcho::Silent,
        );

        // Somebody else holds the port, which is the failure the once-per-streak
        // suppression exists for.
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("occupy a port");
        let addr = occupied.local_addr().expect("the occupied address");
        let shutdown = crate::serve_legs::ServeShutdown::for_watched(true);
        let cell = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut streak = None;
        let mut tasks = tokio::task::JoinSet::new();
        let status_lane = crate::serve_legs::LegStatus::new(handle);

        super::apply_leg_command(
            crate::serve_legs::LegCommand::Bind(addr),
            &mut tasks,
            &shutdown,
            &axum::Router::new(),
            &crate::console::Console::noop(),
            &status_lane,
            &cell,
            &mut streak,
        )
        .await;
        assert_eq!(streak, Some(addr), "the bind really did fail");
        status_lane.flush_due(std::time::Instant::now() + crate::serve_legs::LEG_SETTLE_DWELL);

        svc.drain_requests(&mut engine);
        let status = drain_one_leg_status(&engine);
        assert_eq!(status.tone, dux_core::statusline::StatusTone::Warning);
        assert!(
            status.message.contains(&addr.to_string()),
            "the warning names the address that would not bind: {}",
            status.message
        );
    }

    /// The one leg status on the lane, or a panic naming what came instead.
    fn drain_one_leg_status(engine: &dux_core::engine::Engine) -> dux_core::engine::StatusUpdate {
        let posted = engine.worker_rx.try_recv().expect("a status on the lane");
        let dux_core::worker::WorkerEvent::PollerStatus(status) = posted else {
            panic!("the leg's news rides the poller-status lane");
        };
        assert_eq!(
            status.key.as_deref(),
            Some(crate::serve_legs::TAILSCALE_LEG_KEY)
        );
        status
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
        let console = crate::console::Console::capture(dux_core::activity::ActivityRing::default());
        let cell = std::sync::Arc::new(std::sync::Mutex::new(Some(ts)));
        let mut streak = Some(ts);
        let mut tasks = tokio::task::JoinSet::new();

        super::apply_leg_command(
            crate::serve_legs::LegCommand::Unbind(ts),
            &mut tasks,
            &shutdown,
            &axum::Router::new(),
            &console,
            &crate::serve_legs::LegStatus::default(),
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
        let console = crate::console::Console::capture(dux_core::activity::ActivityRing::default());

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
            crate::serve_legs::LegStatus::default(),
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
        let ring = dux_core::activity::ActivityRing::new(10);
        let console = crate::console::Console::capture(ring.clone());
        console.client_connected("10.0.0.7".parse().unwrap());
        assert_eq!(ring.connections(), 1);
        assert_eq!(ring.snapshot().lines.len(), 1);
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
        // address in the error message (the explicit-failure tenet: the operator
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

    // ── One log, two surfaces ──────────────────────────────────────────────

    /// Everything a serve says over its life, in the order a run says it: the
    /// warnings raised before binding, the banner, a client, its requests, a
    /// Tailscale leg arriving and failing, and the shutdown. Driven through the
    /// same calls `run_plain_http` and `serve_with_engine` make.
    fn a_whole_run(console: &crate::console::Console) {
        let notes = [
            "Tailscale not detected (the tailscale CLI is not installed), so dux is serving on \
             loopback only for now."
                .to_string(),
        ];
        for warning in &notes {
            console.warn(warning);
        }
        let bind_warning = tailscale_bind_warning(
            addr("100.64.0.1:3890"),
            &std::io::Error::new(std::io::ErrorKind::AddrInUse, "address already in use"),
        );
        console.banner(&super::serve_banner(
            "v1.2.3",
            &[(addr("127.0.0.1:3890"), true)],
            &[bind_warning],
            TailscaleMode::Auto,
            true,
        ));
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        console.client_connected(ip);
        console.access("GET", "/api/v1/build", 200, 3);
        console.access("GET", "/nope", 404, 1);
        console.leg_changed("Tailscale address 100.64.0.1:3890 is serving again.");
        console.bind_degraded("the Tailscale listener on 100.64.0.1:3890 stopped serving: gone");
        console.client_disconnected(ip);
        console.info(&dux_core::engine::format_shutdown_start(
            1,
            0,
            std::time::Duration::from_secs(30),
        ));
        console.error(super::FORCE_EXIT_MESSAGE);
    }

    /// The user asked for the two logs to match completely: `dux server`'s stdout
    /// (colors stripped) and the flip's viewer must hold the same lines, word for
    /// word, for the same run.
    #[test]
    fn dux_server_and_the_flip_log_the_same_lines_for_the_same_run() {
        let (stdout, sink) = crate::console::Console::test_capture(true);
        let ring = dux_core::activity::ActivityRing::new(1000);
        let flip = crate::console::Console::test_ring_capture(ring.clone());
        a_whole_run(&stdout);
        a_whole_run(&flip);

        let printed: Vec<String> = sink
            .contents()
            .lines()
            .map(crate::console::strip_ansi)
            .collect();
        let viewed: Vec<String> = ring
            .snapshot()
            .lines
            .iter()
            .map(dux_core::serve_log::LogLine::text)
            .collect();
        assert_eq!(printed, viewed);

        // And the run really said everything it should: the startup warning,
        // the banner with its bind-failure and waiting rows, the access log.
        let joined = viewed.join("\n");
        for expected in [
            "12:00:00 \u{26a0} Tailscale not detected",
            "dux v1.2.3  plain HTTP",
            "  \u{279c} Local (loopback): http://127.0.0.1:3890",
            "  \u{26a0} could not bind the Tailscale address 100.64.0.1:3890",
            "  \u{26a0} Tailscale: the interface is here, but its address would not bind",
            "12:00:00 GET /api/v1/build 200 3ms",
            "12:00:00 \u{279c} client connected from 127.0.0.1",
            "12:00:00 \u{279c} Requesting 1 agent and 0 terminals to gracefully shut down",
            "12:00:00 \u{2717} second interrupt received during shutdown",
        ] {
            assert!(
                joined.contains(expected),
                "missing {expected:?} in:\n{joined}"
            );
        }
    }

    #[test]
    fn the_banner_waits_for_the_interface_when_no_address_was_detected() {
        let banner = super::serve_banner(
            "v1",
            &[(addr("127.0.0.1:3890"), true)],
            &[],
            TailscaleMode::Auto,
            false,
        );
        assert!(
            banner
                .warnings
                .iter()
                .any(|w| w.starts_with("Tailscale: waiting for the interface")),
            "{banner:?}"
        );
        assert_eq!(
            banner.security_note.as_deref(),
            Some(super::SAFETY_NOTE_TAILNET_WATCHED)
        );
    }

    #[test]
    fn the_banner_labels_a_bound_tailscale_leg_and_waits_for_nothing() {
        let banner = super::serve_banner(
            "v1",
            &[
                (addr("127.0.0.1:3890"), true),
                (addr("100.64.0.1:3890"), false),
            ],
            &[],
            TailscaleMode::Yes,
            true,
        );
        let labels: Vec<&str> = banner.listeners.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, vec!["Local (loopback)", "Tailscale"]);
        assert!(!banner.warnings.iter().any(|w| w.contains("waiting")));
        assert_eq!(
            banner.security_note.as_deref(),
            Some(super::SAFETY_NOTE_TAILNET)
        );
    }

    // ── A second stop signal ──────────────────────────────────────────────

    /// In the flip, a second Ctrl-C mid-shutdown gives the terminal back BEFORE
    /// the process exits (an exit runs no destructor, so nothing else would),
    /// and logs the same line `dux server` prints.
    #[test]
    fn a_forced_exit_restores_the_terminal_before_exiting() {
        let ring = dux_core::activity::ActivityRing::new(10);
        let console = crate::console::Console::test_ring_capture(ring.clone());
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let restore: super::RestoreTerminal = {
            let events = events.clone();
            std::sync::Arc::new(move || events.lock().unwrap().push("restore".to_string()))
        };
        let quit = settled_quit();
        let force = super::ForceExit::new(console, Some(restore), quit);
        force.run(|code| events.lock().unwrap().push(format!("exit {code}")));
        assert_eq!(
            *events.lock().unwrap(),
            vec!["restore".to_string(), "exit 130".to_string()]
        );
        let lines: Vec<String> = ring
            .snapshot()
            .lines
            .iter()
            .map(dux_core::serve_log::LogLine::text)
            .collect();
        assert_eq!(
            lines,
            vec![format!("12:00:00 \u{2717} {}", super::FORCE_EXIT_MESSAGE)]
        );
    }

    /// In `dux server`, the line is on stdout before the exit: the console is
    /// flushed first, so the writer thread cannot lose it.
    /// The hatch has to exit even when stdout has stopped draining (`dux server
    /// | less` that nobody is reading): its flush is bounded.
    #[test]
    fn a_forced_exit_exits_even_when_stdout_is_stuck() {
        let console = crate::console::Console::test_stuck_writer();
        let force = super::ForceExit::new(console, None, settled_quit());
        let (exited_tx, exited_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            force.run(|code| {
                let _ = exited_tx.send(code);
            });
        });
        assert_eq!(
            exited_rx.recv_timeout(std::time::Duration::from_secs(10)),
            Ok(130)
        );
    }

    /// A start that fails while loading still shows what it had to warn about
    /// (the non-loopback alarm among them), on stderr.
    #[test]
    fn a_failed_bootstrap_still_prints_the_startup_warnings() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let file_root = tmp.path().join("not-a-dir");
        std::fs::write(&file_root, "x").unwrap();
        let paths = DuxPaths {
            root: file_root.clone(),
            config_path: file_root.join("config.toml"),
            sessions_db_path: file_root.join("sessions.sqlite3"),
            worktrees_root: file_root.join("worktrees"),
            lock_path: file_root.join("dux.lock"),
        };
        let mut err = Vec::new();
        let result = super::bootstrap_or_report(
            &paths,
            &[
                // Printed to stderr before anything loaded: not again.
                super::StartupWarning {
                    text: "dux is binding 0.0.0.0:3890, a non-loopback address".to_string(),
                    already_on_stderr: true,
                },
                super::StartupWarning {
                    text: "Tailscale not detected.".to_string(),
                    already_on_stderr: false,
                },
            ],
            &mut err,
        );
        assert!(result.is_err());
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "WARNING: Tailscale not detected.\n",
            "the alarm is on stderr once, from before the load"
        );
    }

    /// A quit force with no wind-down under way, so a forced exit does not wait.
    fn settled_quit() -> std::sync::Arc<super::QuitForce> {
        std::sync::Arc::new(super::QuitForce::default())
    }

    /// Both modes wind their children down through one function, so a forced
    /// quit logs the same lines in both: the start, the force line at the
    /// moment it was asked for, then the result saying what was force-closed
    /// (a warning, since something did not stop in time).
    #[test]
    fn a_forced_quit_logs_the_same_lines_in_either_mode() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let root = tmp.path().to_path_buf();
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        let mut engine = crate::test_support::bootstrap_test_engine(&paths).expect("engine");
        engine.config.terminal.command = "sh".to_string();
        engine.config.terminal.args = vec![
            "-c".to_string(),
            "trap '' TERM HUP; exec sleep 120".to_string(),
        ];
        engine
            .create_standalone_terminal(24, 80)
            .expect("a terminal to wind down");
        // Let the shell install its trap before the SIGTERM arrives.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let ring = dux_core::activity::ActivityRing::new(100);
        let console = crate::console::Console::test_ring_capture(ring.clone());
        let quit = std::sync::Arc::new(super::QuitForce::default());
        {
            let quit = quit.clone();
            let console = console.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                assert!(quit.request(&console), "the first ask logs the line");
                assert!(!quit.request(&console), "a second ask logs nothing");
            });
        }
        let started = std::time::Instant::now();
        let report = super::wind_down_children(
            &mut engine,
            &console,
            std::time::Duration::from_secs(60),
            &quit,
            |_| {},
            || {},
        )
        .expect("there was a child to wind down");
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        assert!(report.timed_out, "{report:?}");
        let lines: Vec<String> = ring
            .snapshot()
            .lines
            .iter()
            .map(dux_core::serve_log::LogLine::text)
            .collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].starts_with("12:00:00 \u{279c} Requesting 0 agents and 1 terminal"));
        assert_eq!(
            lines[1],
            format!("12:00:00 \u{2717} {}", super::FORCE_EXIT_MESSAGE)
        );
        assert!(
            lines[2].starts_with("12:00:00 \u{26a0} 0 agents and 0 terminals exited"),
            "{}",
            lines[2]
        );
    }

    /// In the flip a forced quit wins over a serve that also failed: the process
    /// still exits 130 with the force line, and the failure goes to `dux.log`.
    #[test]
    fn a_forced_quit_wins_over_a_serve_error() {
        assert!(matches!(
            super::flip_outcome(
                super::ServerExit::ForceQuit(super::ForceQuitHandle::default()),
                Some(anyhow::anyhow!("boom"))
            ),
            Ok(super::ServerExit::ForceQuit(_))
        ));
        assert!(
            super::flip_outcome(
                super::ServerExit::QuitProcess,
                Some(anyhow::anyhow!("boom"))
            )
            .is_err()
        );
        assert!(matches!(
            super::flip_outcome(super::ServerExit::ReturnToTui, None),
            Ok(super::ServerExit::ReturnToTui)
        ));
    }

    /// A forced exit waits for the children to be killed, but never for long:
    /// when nothing settles (the shutdown never started), it exits anyway.
    #[test]
    fn a_forced_exit_does_not_wait_forever_for_the_children() {
        let (console, _sink) = crate::console::Console::test_capture(false);
        let quit = std::sync::Arc::new(super::QuitForce::default());
        // The wind-down started and never finished.
        quit.start();
        let force = super::ForceExit::new(console, None, quit)
            .with_settle_bound(std::time::Duration::from_millis(200));
        let started = std::time::Instant::now();
        let mut code = None;
        force.run(|c| code = Some(c));
        assert_eq!(code, Some(130));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// A second Ctrl-c before anything is
    /// being wound down does not sit out the settle bound.
    #[test]
    fn a_second_ctrl_c_before_the_wind_down_exits_at_once() {
        let (console, _sink) = crate::console::Console::test_capture(false);
        let quit = std::sync::Arc::new(super::QuitForce::default());
        let force = super::ForceExit::new(console, None, quit)
            .with_settle_bound(std::time::Duration::from_secs(10));
        let started = std::time::Instant::now();
        let mut code = None;
        force.run(|c| code = Some(c));
        assert_eq!(code, Some(130));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "took {:?}",
            started.elapsed()
        );
    }

    /// Exiting after a forced quit drops the engine first, so a config write
    /// still queued (a lazy save) lands on disk rather than dying with the
    /// process.
    #[test]
    fn a_forced_quit_lets_pending_config_writes_land() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let root = tmp.path().to_path_buf();
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        let engine = crate::test_support::bootstrap_test_engine(&paths).expect("engine");
        let mut config = engine.config.clone();
        config.server.port = 4567;
        engine.config_writer.save_lazy(config);
        let mut code = None;
        let handle = super::ForceQuitHandle::default();
        super::finish_forced_quit(engine, &handle, |c| code = Some(c));
        assert_eq!(code, Some(130));
        let saved = std::fs::read_to_string(&paths.config_path).expect("config written");
        assert!(saved.contains("port = 4567"), "{saved}");
    }

    /// When the signal hatch already took the exit (the owner was too slow),
    /// the owner still drops the engine but never calls exit a second time.
    #[test]
    fn the_flip_owner_leaves_the_exit_to_a_hatch_that_took_it() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let root = tmp.path().to_path_buf();
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        let engine = crate::test_support::bootstrap_test_engine(&paths).expect("engine");
        let handle = super::ForceQuitHandle::default();
        assert!(handle.0.claim_exit(), "the hatch claims first");
        let mut code = None;
        super::finish_forced_quit(engine, &handle, |c| code = Some(c));
        assert_eq!(code, None, "only one thread exits");
    }

    /// A second stop signal while the wind-down runs hands the exit to the
    /// engine's owner, which drops the engine (flushing config writes) and
    /// exits 130; the hatch exits only if the owner does not get there in time.
    #[test]
    fn a_second_signal_during_the_wind_down_hands_the_exit_to_the_owner() {
        let (console, _sink) = crate::console::Console::test_capture(false);
        let quit = std::sync::Arc::new(super::QuitForce::default());
        quit.start();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let owner = {
            let quit = quit.clone();
            let events = events.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if quit.claim_exit() {
                    events.lock().unwrap().push("owner exits 130".to_string());
                }
            })
        };
        let force = super::ForceExit::new(console, None, quit)
            .with_settle_bound(std::time::Duration::from_secs(5));
        force.run(|c| events.lock().unwrap().push(format!("hatch exits {c}")));
        owner.join().unwrap();
        assert_eq!(*events.lock().unwrap(), vec!["owner exits 130".to_string()]);
    }

    /// The hatch waits for the owner on a thread of its own, never on a runtime
    /// worker: in the flip the owner's way out tears that runtime down (bounded
    /// at 2 s), and a worker parked in the hatch's wait would eat that budget
    /// and leave the owner late.
    #[test]
    fn the_hatch_does_not_hold_the_runtime_while_it_waits() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (console, _sink) = crate::console::Console::test_capture(false);
        let quit = std::sync::Arc::new(super::QuitForce::default());
        quit.start();
        let force = super::ForceExit::new(console, None, quit.clone())
            .with_settle_bound(std::time::Duration::from_secs(5));
        let (exited_tx, exited_rx) = std::sync::mpsc::channel();
        runtime.spawn(async move {
            super::fire_force_exit(force, move |code| {
                let _ = exited_tx.send(code);
            });
        });
        // Let the hatch get into its wait.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let started = std::time::Instant::now();
        runtime.shutdown_timeout(std::time::Duration::from_secs(2));
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "the runtime tore down in {:?}",
            started.elapsed()
        );
        // The owner gets there well inside the bound and takes the exit.
        assert!(quit.claim_exit());
        assert!(
            exited_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .is_err(),
            "the hatch saw the owner take the exit and did not exit too"
        );
    }

    /// `dux server`'s owner side of that hand-over: after the engine thread
    /// has run the forced wind-down it is joined (so the engine is dropped and
    /// its queued config writes land) before the process exits 130.
    #[tokio::test(flavor = "multi_thread")]
    async fn dux_server_drops_its_engine_before_a_forced_exit() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let root = tmp.path().to_path_buf();
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        let engine = crate::test_support::bootstrap_test_engine(&paths).expect("engine");
        let mut config = engine.config.clone();
        config.server.port = 4568;
        engine.config_writer.save_lazy(config);
        let (console, _sink) = crate::console::Console::test_capture(false);
        let quit = std::sync::Arc::new(super::QuitForce::default());
        let (handle, join) = crate::engine_actor::spawn_engine_thread_with_console(
            engine,
            console.clone(),
            quit.clone(),
        );
        quit.request(&console);
        handle.shutdown().await;
        drop(handle);
        let outcome = tokio::task::spawn_blocking({
            let quit = quit.clone();
            move || {
                super::finish_engine_thread(
                    join,
                    &quit,
                    &console,
                    std::time::Duration::from_secs(5),
                )
            }
        })
        .await
        .unwrap();
        assert_eq!(outcome, super::OwnerExit::Forced);
        let saved = std::fs::read_to_string(&paths.config_path).expect("config written");
        assert!(saved.contains("port = 4568"), "{saved}");
    }

    /// Lines logged before an early failure (the runtime refusing to build)
    /// still reach stdout: the flush covers every way out.
    #[test]
    fn an_early_failure_still_flushes_what_was_logged() {
        let (console, sink) = crate::console::Console::test_capture(false);
        let result: anyhow::Result<()> = super::flushing(&console, || {
            console.warn("a startup warning");
            Err(anyhow::anyhow!("the runtime would not build"))
        });
        assert!(result.is_err());
        assert!(sink.raw_contents().contains("a startup warning"));
    }

    #[test]
    fn a_forced_exit_flushes_the_console_before_exiting() {
        let (console, sink) = crate::console::Console::test_capture(false);
        let force = super::ForceExit::new(console, None, settled_quit());
        let printed_at_exit = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        force.run(|_| {
            // Read straight from the buffer, without the test's own barrier,
            // so only the run's own flush can have put the line there.
            *printed_at_exit.lock().unwrap() = sink.raw_contents();
        });
        assert_eq!(
            *printed_at_exit.lock().unwrap(),
            format!("12:00:00 error {}\n", super::FORCE_EXIT_MESSAGE)
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
        let warnings = vec!["Tailscale: 100.64.0.1:8080 busy, serving without it".to_string()];
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
        // referenced: a dead-code construction wouldn't catch API drift.
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
        legs: tokio::sync::mpsc::Sender<(u64, WatchEvent)>,
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
            Self::start_inner(
                mode,
                forced_no,
                primary,
                initial_leg,
                detect,
                None,
                Console::noop(),
            )
        }

        fn start_inner(
            mode: TailscaleMode,
            forced_no: bool,
            primary: Option<SocketAddr>,
            initial_leg: Option<SocketAddr>,
            detect: Arc<dyn Fn() -> Result<IpAddr, TailscaleUnavailable> + Send + Sync>,
            identify: Option<crate::IdentityProbe>,
            console: Console,
        ) -> Self {
            Self::start_inner_every(
                mode,
                forced_no,
                primary,
                initial_leg,
                detect,
                identify,
                console,
                FAST_LOOK,
                false,
            )
        }

        /// A harness whose loop reads names through `identify`, prints to
        /// `console`, and believes dux runs in a container or not.
        fn start_in_container(
            identify: crate::IdentityProbe,
            console: Console,
            in_container: bool,
        ) -> Self {
            Self::start_inner_every(
                TailscaleMode::Auto,
                false,
                None,
                None,
                Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
                Some(identify),
                console,
                FAST_LOOK,
                in_container,
            )
        }

        #[allow(clippy::too_many_arguments)]
        fn start_inner_every(
            mode: TailscaleMode,
            forced_no: bool,
            primary: Option<SocketAddr>,
            initial_leg: Option<SocketAddr>,
            detect: Arc<dyn Fn() -> Result<IpAddr, TailscaleUnavailable> + Send + Sync>,
            identify: Option<crate::IdentityProbe>,
            console: Console,
            period: Duration,
            in_container: bool,
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
                TailscaleLoop::new(mode, forced_no, primary, initial_leg, &control, detect)
                    .in_container(in_container);
            if let Some(identify) = identify {
                ts = ts.with_identity(identify, None).with_watch_period(period);
            }
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
                console,
                LegStatus::default(),
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

        /// A harness whose loop reads names through `identify`, the way both
        /// serve paths wire it, with no initial identity (the background and
        /// flip serves, which may not wait on the CLI as they start).
        fn start_with_identity(
            mode: TailscaleMode,
            primary: Option<SocketAddr>,
            detect: Arc<dyn Fn() -> Result<IpAddr, TailscaleUnavailable> + Send + Sync>,
            identify: crate::IdentityProbe,
        ) -> Self {
            Self::start_inner(
                mode,
                false,
                primary,
                None,
                detect,
                Some(identify),
                Console::noop(),
            )
        }

        /// As [`Self::start_with_identity`], looking every `period`.
        fn start_with_identity_every(
            mode: TailscaleMode,
            primary: Option<SocketAddr>,
            identify: crate::IdentityProbe,
            period: Duration,
        ) -> Self {
            Self::start_inner_every(
                mode,
                false,
                primary,
                None,
                Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
                Some(identify),
                Console::noop(),
                period,
                false,
            )
        }

        /// As [`Self::start_with_identity`], printing to `console`, which is
        /// how a test watches the QR codes the loop shows.
        fn start_with_console(
            mode: TailscaleMode,
            primary: Option<SocketAddr>,
            identify: crate::IdentityProbe,
            console: Console,
        ) -> Self {
            Self::start_inner(
                mode,
                false,
                primary,
                None,
                Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
                Some(identify),
                console,
            )
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

    // ── This machine's tailnet name, through the real router ──────────────

    fn named(name: &str, serve: &[(&str, bool)]) -> TailscaleIdentity {
        TailscaleIdentity {
            status: dux_core::tailscale::SelfStatus {
                dns_name: Some(name.to_string()),
                magic_dns_enabled: true,
                magic_dns_suffix: name.split_once('.').map(|(_, suffix)| suffix.to_string()),
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
            forward_to_dux: false,
        }
    }

    /// A raw TCP Funnel forwarding to dux's port.
    fn tcp_funnelled(name: &str) -> TailscaleIdentity {
        TailscaleIdentity {
            funnel: true,
            funnel_to_dux: true,
            ..named(name, &[])
        }
    }

    /// A Funnel this machine has to ANOTHER port may be the operator's own
    /// relay to dux, and that deliberate setup is theirs: dux keeps serving,
    /// withdraws its MagicDNS name and warns, but does not lock.
    #[tokio::test]
    async fn a_funnel_to_another_port_withdraws_the_name_but_never_locks() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let elsewhere = TailscaleIdentity {
            funnel: true,
            funnel_to_dux: false,
            ..named("box.tail.ts.net", &[])
        };
        let (identify, _slot) = scripted_identity(elsewhere);
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        until_status(&app, "localhost", StatusCode::OK).await;
        tokio::time::sleep(FAST_LOOK * 10).await;
        assert_eq!(
            h.control.funnel_lockout().get(),
            crate::host_guard::FunnelLockout::Open
        );
        assert_eq!(status_for(&app, "localhost").await, StatusCode::OK);
        assert_eq!(
            status_for(&app, "box.tail.ts.net").await,
            StatusCode::FORBIDDEN,
            "the name stays withdrawn while any Funnel is on"
        );
        h.finish().await;
    }

    #[tokio::test]
    async fn a_tcp_funnel_to_dux_refuses_every_request_until_it_goes() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, slot) = scripted_identity(tcp_funnelled("box.tail.ts.net"));
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        // What a raw TCP stream through Funnel can claim: loopback, or the
        // tailnet literal. Neither carries a Funnel marker.
        until_status(&app, "localhost", StatusCode::SERVICE_UNAVAILABLE).await;
        assert_eq!(
            status_for(&app, "100.101.102.103:3890").await,
            StatusCode::SERVICE_UNAVAILABLE
        );

        // A look that fails does not lift it: fail closed.
        *slot.lock().unwrap() = Err(TailscaleUnavailable::CommandFailed);
        tokio::time::sleep(FAST_LOOK * 10).await;
        assert_eq!(
            status_for(&app, "localhost").await,
            StatusCode::SERVICE_UNAVAILABLE
        );

        // The Funnel goes away and dux serves again by itself.
        *slot.lock().unwrap() = Ok(named("box.tail.ts.net", &[]));
        until_status(&app, "localhost", StatusCode::OK).await;
        assert_eq!(
            status_for(&app, "100.101.102.103:3890").await,
            StatusCode::OK
        );
        h.finish().await;
    }

    /// The Tailscale CLI as a test holds it: the first look waits until the
    /// test releases it with an answer.
    fn parked_identity() -> (
        crate::IdentityProbe,
        std::sync::mpsc::Sender<Result<TailscaleIdentity, TailscaleUnavailable>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let rx = std::sync::Mutex::new(rx);
        let last = std::sync::Mutex::new(None);
        let probe: crate::IdentityProbe = Arc::new(move || {
            let mut last = last.lock().unwrap();
            if let Ok(answer) = rx.lock().unwrap().try_recv() {
                *last = Some(answer);
            }
            while last.is_none() {
                match rx.lock().unwrap().recv_timeout(Duration::from_millis(20)) {
                    Ok(answer) => *last = Some(answer),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(TailscaleUnavailable::CommandFailed);
                    }
                }
            }
            last.clone().expect("set above")
        });
        (probe, tx)
    }

    #[tokio::test]
    async fn nothing_is_served_until_the_first_funnel_check_lands() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, answer) = parked_identity();
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        assert_eq!(
            status_for(&app, "localhost").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        answer.send(Ok(named("box.tail.ts.net", &[]))).unwrap();
        until_status(&app, "localhost", StatusCode::OK).await;
        h.finish().await;
    }

    #[tokio::test]
    async fn no_tailscale_cli_or_no_daemon_means_no_funnel_and_nothing_is_refused() {
        use axum::http::StatusCode;
        // The two answers: no Tailscale here at all, and a daemon the CLI says
        // is not running. A daemon it merely cannot reach is not one of them.
        for reason in [
            TailscaleUnavailable::CommandMissing,
            TailscaleUnavailable::DaemonStopped,
        ] {
            let tmp = dux_core::test_scratch::ScratchDir::new();
            let (_primary, primary_addr) = primary_listener();
            let (identify, _slot) = scripted_identity(named("box.tail.ts.net", &[]));
            let identify: crate::IdentityProbe = {
                let reason = reason.clone();
                let _ = identify;
                Arc::new(move || Err(reason.clone()))
            };
            let h = Harness::start_with_identity(
                TailscaleMode::Auto,
                Some(primary_addr),
                Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
                identify,
            );
            let app = router_over(&h, tmp.path());
            until_status(&app, "localhost", StatusCode::OK).await;
            h.finish().await;
        }
    }

    #[tokio::test]
    async fn a_cli_that_keeps_failing_refuses_everything_until_a_look_succeeds() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, slot) = scripted_identity(named("box.tail.ts.net", &[]));
        *slot.lock().unwrap() = Err(TailscaleUnavailable::CommandFailed);
        let h = Harness::start_with_identity(
            TailscaleMode::Yes,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        tokio::time::sleep(FAST_LOOK * 10).await;
        assert_eq!(
            status_for(&app, "localhost").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        *slot.lock().unwrap() = Ok(named("box.tail.ts.net", &[]));
        until_status(&app, "localhost", StatusCode::OK).await;
        h.finish().await;
    }

    #[tokio::test]
    async fn a_web_funnel_to_dux_refuses_localhost_and_the_tailnet_literal() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let web_funnel = TailscaleIdentity {
            funnel_to_dux: true,
            ..named("box.tail.ts.net", &[("https://box.tail.ts.net", true)])
        };
        let (identify, _slot) = scripted_identity(web_funnel);
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        tokio::time::sleep(FAST_LOOK * 10).await;
        for host in ["localhost", "100.101.102.103:3890", "box.tail.ts.net"] {
            assert_eq!(
                status_for(&app, host).await,
                StatusCode::SERVICE_UNAVAILABLE,
                "{host}"
            );
        }
        h.finish().await;
    }

    #[tokio::test]
    async fn a_serve_with_no_primary_address_still_checks_for_funnel() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (identify, _slot) = scripted_identity(named("box.tail.ts.net", &[]));
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            None,
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        until_status(&app, "localhost", StatusCode::OK).await;
        h.finish().await;
    }

    #[tokio::test]
    async fn switching_to_no_lifts_a_refusal_out_loud() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let ring = dux_core::activity::ActivityRing::new(2000);
        let (identify, _slot) = scripted_identity(tcp_funnelled("box.tail.ts.net"));
        let h = Harness::start_with_console(
            TailscaleMode::Auto,
            Some(primary_addr),
            identify,
            Console::capture(ring.clone()),
        );
        let app = router_over(&h, tmp.path());
        // Wait for the Funnel verdict itself, not just a refusal (a refusal is
        // also what the checking state answers before the first look lands).
        let deadline = tokio::time::Instant::now() + WAIT;
        while h.control.funnel_lockout().get() != crate::host_guard::FunnelLockout::Funnel {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the Funnel was never seen"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            status_for(&app, "localhost").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        h.control.set_mode(TailscaleMode::No).await;
        until_status(&app, "localhost", StatusCode::OK).await;
        let said = ring_texts(&ring);
        assert!(
            said.iter().any(|m| m.contains("no longer checks")),
            "{said:?}"
        );
        h.finish().await;
    }

    /// Once a Funnel to dux has been seen, only a look that SEES it gone lifts
    /// the refusal: a daemon that stops (so nothing can be checked) keeps it.
    #[tokio::test]
    async fn a_funnel_lockout_outlives_a_daemon_outage_until_a_look_sees_it_gone() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, slot) = scripted_identity(tcp_funnelled("box.tail.ts.net"));
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        let deadline = tokio::time::Instant::now() + WAIT;
        while h.control.funnel_lockout().get() != crate::host_guard::FunnelLockout::Funnel {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the Funnel was never seen"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        *slot.lock().unwrap() = Err(TailscaleUnavailable::DaemonStopped);
        tokio::time::sleep(FAST_LOOK * 10).await;
        assert_eq!(
            status_for(&app, "localhost").await,
            StatusCode::SERVICE_UNAVAILABLE,
            "the daemon stopped, but nothing has seen the Funnel go"
        );
        *slot.lock().unwrap() = Ok(named("box.tail.ts.net", &[]));
        until_status(&app, "localhost", StatusCode::OK).await;
        h.finish().await;
    }

    #[tokio::test]
    async fn a_failed_look_on_choosing_yes_again_withdraws_the_name() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, slot) = scripted_identity(named("box.tail.ts.net", &[]));
        // A watcher that never gets to its second look, so only the one-shot
        // look that choosing `yes` runs can withdraw the name.
        let h = Harness::start_with_identity_every(
            TailscaleMode::Yes,
            Some(primary_addr),
            identify,
            Duration::from_secs(3600),
        );
        let app = router_over(&h, tmp.path());
        until_status(&app, "box.tail.ts.net", StatusCode::OK).await;
        *slot.lock().unwrap() = Err(TailscaleUnavailable::CommandFailed);
        h.control.set_mode(TailscaleMode::Yes).await;
        // The name is withdrawn, and with the Funnel state unknown nothing at
        // all is served until a look succeeds.
        assert!(h.control.own_magicdns_name().snapshot().is_empty());
        assert_eq!(
            status_for(&app, "box.tail.ts.net").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        h.finish().await;
    }

    #[tokio::test]
    async fn a_watcher_that_will_not_start_takes_the_startup_name_with_it() {
        // Nothing would watch the name's Funnel state, so nothing may admit it.
        let (_primary, primary_addr) = primary_listener();
        let (control, _mode_rx) = TailscaleModeControl::new(
            tokio::runtime::Handle::current(),
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(true)),
        );
        let (identify, _slot) = scripted_identity(named("box.tail.ts.net", &[]));
        let mut ts = TailscaleLoop::new(
            TailscaleMode::Auto,
            false,
            Some(primary_addr),
            None,
            &control,
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
        )
        .with_identity(identify, Some(named("box.tail.ts.net", &[])))
        .refusing_watcher_threads();
        assert_eq!(
            control.own_magicdns_name().snapshot(),
            vec!["box.tail.ts.net".to_string()],
            "precondition: the startup look admitted it"
        );
        ts.start_watcher_if_wanted();
        assert!(control.own_magicdns_name().snapshot().is_empty());
    }

    /// Funnel on for something dux does not show (a TCP forward, another port).
    fn funnelled_elsewhere(name: &str) -> TailscaleIdentity {
        TailscaleIdentity {
            funnel: true,
            ..named(name, &[])
        }
    }

    /// The period an identity harness's watcher looks at, so a test of what a
    /// LATER look finds takes milliseconds.
    const FAST_LOOK: Duration = Duration::from_millis(20);

    #[tokio::test]
    async fn on_yes_a_funnel_switched_on_later_withdraws_the_name() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, slot) = scripted_identity(named("box.tail.ts.net", &[]));
        let h = Harness::start_with_identity(
            TailscaleMode::Yes,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        until_status(&app, "box.tail.ts.net", StatusCode::OK).await;

        // `yes` looks for the ADDRESS once, but the name's Funnel state is
        // watched for as long as the guard may admit it.
        *slot.lock().unwrap() = Ok(funnelled_elsewhere("box.tail.ts.net"));
        until_status(&app, "box.tail.ts.net", StatusCode::FORBIDDEN).await;

        *slot.lock().unwrap() = Ok(named("box.tail.ts.net", &[]));
        until_status(&app, "box.tail.ts.net", StatusCode::OK).await;
        h.finish().await;
    }

    #[tokio::test]
    async fn a_funnel_to_anything_withdraws_the_name_on_auto() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, slot) = scripted_identity(named("box.tail.ts.net", &[]));
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        until_status(&app, "box.tail.ts.net", StatusCode::OK).await;
        *slot.lock().unwrap() = Ok(funnelled_elsewhere("box.tail.ts.net"));
        until_status(&app, "box.tail.ts.net", StatusCode::FORBIDDEN).await;
        h.finish().await;
    }

    #[tokio::test]
    async fn a_failed_lookup_withdraws_the_name_until_one_succeeds() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, slot) = scripted_identity(named("box.tail.ts.net", &[]));
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        until_status(&app, "box.tail.ts.net", StatusCode::OK).await;

        // A look that fails after one succeeded leaves the Funnel state unknown,
        // so everything is refused until a look succeeds, the name included.
        *slot.lock().unwrap() = Err(TailscaleUnavailable::CommandFailed);
        until_status(&app, "box.tail.ts.net", StatusCode::SERVICE_UNAVAILABLE).await;
        assert_eq!(
            status_for(&app, "localhost").await,
            StatusCode::SERVICE_UNAVAILABLE
        );

        // The same answer as before comes back, and is admitted again: an
        // unconfirmed identity is not "unchanged".
        *slot.lock().unwrap() = Ok(named("box.tail.ts.net", &[]));
        until_status(&app, "box.tail.ts.net", StatusCode::OK).await;
        h.finish().await;
    }

    #[tokio::test]
    async fn a_name_tailscale_did_not_assign_is_never_served_by_itself() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let mut headscale = named("box.vpn.example.com", &[]);
        headscale.status.magic_dns_suffix = Some("vpn.example.com".to_string());
        let (identify, _slot) = scripted_identity(headscale);
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        // Give the watcher a few looks before asserting the refusal holds.
        tokio::time::sleep(FAST_LOOK * 10).await;
        assert_eq!(
            status_for(&app, "box.vpn.example.com").await,
            StatusCode::FORBIDDEN
        );
        h.finish().await;
    }

    /// The Tailscale CLI as the tests script it: whatever identity the test
    /// last put in the slot. Nothing else about the serve is faked.
    fn scripted_identity(
        first: TailscaleIdentity,
    ) -> (
        crate::IdentityProbe,
        Arc<std::sync::Mutex<Result<TailscaleIdentity, TailscaleUnavailable>>>,
    ) {
        let slot = Arc::new(std::sync::Mutex::new(Ok(first)));
        let reader = Arc::clone(&slot);
        let probe: crate::IdentityProbe = Arc::new(move || reader.lock().unwrap().clone());
        (probe, slot)
    }

    /// The real router, built the way both serve paths build it, wired to the
    /// harness's live cells.
    fn router_over(h: &Harness, tmp: &std::path::Path) -> axum::Router {
        crate::server::build_app(
            crate::test_support::test_engine_handle(tmp),
            axum::Router::new(),
            crate::server::RouterParams::plain_http()
                .with_host_allowlist(vec!["127.0.0.1".parse().unwrap()], vec![], false)
                .with_live_tailscale_host_literals(h.control.host_literals())
                .with_live_own_magicdns_name(h.control.own_magicdns_name())
                .with_live_funnel_lockout(h.control.funnel_lockout()),
        )
    }

    async fn status_for(app: &axum::Router, host: &str) -> axum::http::StatusCode {
        use tower::ServiceExt;
        app.clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/healthz")
                    .header("Host", host)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    /// Poll the router until `host` answers `want`, bounded by [`WAIT`].
    async fn until_status(app: &axum::Router, host: &str, want: axum::http::StatusCode) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let got = status_for(app, host).await;
            if got == want {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{host} still answers {got}, wanted {want}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn on_auto_the_watcher_reads_the_name_and_the_router_serves_it_then_follows_a_rename() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, slot) = scripted_identity(named("demo-box.old-tailnet.ts.net", &[]));
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());

        // The watcher parks before its first ADDRESS look, but an unknown name
        // is looked up at once, so this needs no five-second wait.
        until_status(&app, "demo-box.old-tailnet.ts.net:3890", StatusCode::OK).await;
        assert_eq!(
            status_for(&app, "laptop.old-tailnet.ts.net").await,
            StatusCode::FORBIDDEN,
            "another machine on the same tailnet is still refused"
        );

        // The tailnet is renamed. Choosing auto again restarts the watcher with
        // an immediate look, which is the live mode seam doing its job.
        *slot.lock().unwrap() = Ok(named("demo-box.example-tailnet.ts.net", &[]));
        assert_eq!(
            h.control.set_mode(TailscaleMode::Auto).await,
            TailscaleModeOutcome::Applied { bound: None }
        );
        until_status(&app, "demo-box.example-tailnet.ts.net", StatusCode::OK).await;
        until_status(&app, "demo-box.old-tailnet.ts.net", StatusCode::FORBIDDEN).await;
        h.finish().await;
    }

    /// Every line text the flip's ring holds right now.
    fn ring_texts(ring: &dux_core::activity::ActivityRing) -> Vec<String> {
        ring.snapshot()
            .lines
            .iter()
            .map(|line| line.text())
            .collect()
    }

    /// Wait until some line the ring holds contains every one of `needles`.
    async fn until_ring_says(ring: &dux_core::activity::ActivityRing, needles: &[&str]) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let texts = ring_texts(ring);
            if needles.iter().all(|n| texts.iter().any(|t| t.contains(n))) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the log never said {needles:?}: {texts:#?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The start-web-server flip's path end to end: as soon as the name is read
    /// the loop logs the tailnet rows and the QR codes, the same lines
    /// `dux server` prints, and again when a rename changes them.
    #[tokio::test]
    async fn the_tailnet_rows_and_qr_codes_follow_the_name_into_the_flips_log() {
        let (_primary, primary_addr) = primary_listener();
        let ring = dux_core::activity::ActivityRing::new(2000);
        let console = Console::capture(ring.clone());
        console.set_qr_codes(true);
        let (identify, slot) = scripted_identity(named(
            "demo-box.old-tailnet.ts.net",
            &[("https://demo-box.old-tailnet.ts.net", false)],
        ));
        let h =
            Harness::start_with_console(TailscaleMode::Auto, Some(primary_addr), identify, console);
        until_ring_says(
            &ring,
            &[
                "Tailscale (HTTPS, tailscale serve): https://demo-box.old-tailnet.ts.net",
                "Scan to open dux from your phone:",
            ],
        )
        .await;

        *slot.lock().unwrap() = Ok(named(
            "demo-box.example-tailnet.ts.net",
            &[("https://demo-box.example-tailnet.ts.net", false)],
        ));
        h.control.set_mode(TailscaleMode::Auto).await;
        until_ring_says(
            &ring,
            &["Tailscale (HTTPS, tailscale serve): https://demo-box.example-tailnet.ts.net"],
        )
        .await;
        let captions = ring_texts(&ring)
            .iter()
            .filter(|t| t.contains("Scan to open dux"))
            .count();
        assert_eq!(captions, 2, "a fresh pair of codes for the new name");
        h.finish().await;
    }

    /// The flip cannot wait for its first look before serving, so the loop
    /// turns while that look is still out. No QR code is printed until it lands:
    /// a code for the Tailscale IP alone, superseded a moment later by the
    /// pair, is a second set of codes for one serve, and every request answers
    /// "checking" until then anyway.
    #[tokio::test]
    async fn no_qr_code_is_printed_until_the_first_look_lands() {
        let (_primary, primary_addr) = primary_listener();
        let ring = dux_core::activity::ActivityRing::new(2000);
        let console = Console::capture(ring.clone());
        console.set_qr_codes(true);
        let opened = Arc::new(AtomicBool::new(false));
        let gate = Arc::clone(&opened);
        let identify: crate::IdentityProbe = Arc::new(move || {
            while !gate.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(named(
                "demo-box.example-tailnet.ts.net",
                &[("https://demo-box.example-tailnet.ts.net", false)],
            ))
        });
        let leg: SocketAddr = format!("100.101.102.103:{}", primary_addr.port())
            .parse()
            .unwrap();
        let h = Harness::start_inner(
            TailscaleMode::Auto,
            false,
            Some(primary_addr),
            Some(leg),
            Arc::new(move || Ok(leg.ip())),
            Some(identify),
            console,
        );
        tokio::time::sleep(FAST_LOOK * 10).await;
        let early: Vec<String> = ring_texts(&ring)
            .into_iter()
            .filter(|t| t.contains("Scan to open dux"))
            .collect();
        assert!(early.is_empty(), "codes before the first look: {early:?}");

        opened.store(true, Ordering::SeqCst);
        until_ring_says(&ring, &["Scan to open dux from your phone:"]).await;
        tokio::time::sleep(FAST_LOOK * 5).await;
        let captions = ring_texts(&ring)
            .iter()
            .filter(|t| t.contains("Scan to open dux"))
            .count();
        assert_eq!(captions, 1, "one set of codes, the pair");
        h.finish().await;
    }

    fn container_warnings(ring: &dux_core::activity::ActivityRing) -> usize {
        ring_texts(ring)
            .iter()
            .filter(|t| t.contains("inside a container"))
            .count()
    }

    /// Inside a container dux cannot see a Tailscale outside it, so when its
    /// checks find none it serves (that setup is the operator's own) and says
    /// once, at start, what that means. The flip and the background serve take
    /// their first look on the watcher.
    #[tokio::test]
    async fn a_container_with_no_tailscale_in_sight_serves_and_warns_once_at_start() {
        for (reason, in_container, warned) in [
            (TailscaleUnavailable::CommandMissing, true, 1),
            (TailscaleUnavailable::DaemonStopped, true, 1),
            (TailscaleUnavailable::CommandMissing, false, 0),
        ] {
            let ring = dux_core::activity::ActivityRing::new(2000);
            let console = Console::capture(ring.clone());
            let answer = reason.clone();
            let probe: crate::IdentityProbe = Arc::new(move || Err(answer.clone()));
            let h = Harness::start_in_container(probe, console, in_container);
            let deadline = tokio::time::Instant::now() + WAIT;
            while h.control.funnel_lockout().get() != crate::host_guard::FunnelLockout::Open {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "{reason:?} never served"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // Several more looks, all saying the same: still said once.
            tokio::time::sleep(FAST_LOOK * 10).await;
            assert_eq!(
                container_warnings(&ring),
                warned,
                "{reason:?} in_container={in_container}: {:#?}",
                ring_texts(&ring)
            );
            h.finish().await;
        }
    }

    /// A container where Tailscale IS in sight is checked like anywhere else
    /// and says nothing about being in a container.
    #[tokio::test]
    async fn a_container_that_sees_tailscale_says_nothing_about_it() {
        let ring = dux_core::activity::ActivityRing::new(2000);
        let console = Console::capture(ring.clone());
        let (identify, _slot) = scripted_identity(named("box.tail.ts.net", &[]));
        let h = Harness::start_in_container(identify, console, true);
        let deadline = tokio::time::Instant::now() + WAIT;
        while h.control.funnel_lockout().get() != crate::host_guard::FunnelLockout::Open {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(FAST_LOOK * 5).await;
        assert_eq!(container_warnings(&ring), 0);
        h.finish().await;
    }

    /// `dux server` takes its first look itself, before the watcher: the same
    /// warning, through the same console line.
    #[tokio::test]
    async fn dux_servers_startup_look_warns_about_the_container_the_same_way() {
        let ring = dux_core::activity::ActivityRing::new(2000);
        let console = Console::capture(ring.clone());
        let (control, _mode_rx) = TailscaleModeControl::new(
            tokio::runtime::Handle::current(),
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(true)),
        );
        let mut ts = TailscaleLoop::new(
            TailscaleMode::Auto,
            false,
            None,
            None,
            &control,
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
        )
        .in_container(true);
        ts.apply_look(
            Err(TailscaleUnavailable::CommandMissing),
            &console,
            &LegStatus::default(),
        );
        assert_eq!(container_warnings(&ring), 1, "{:#?}", ring_texts(&ring));
        let text = ring_texts(&ring)
            .into_iter()
            .find(|t| t.contains("inside a container"))
            .unwrap();
        for needle in ["no login", "outside this container", "terminals", "private"] {
            assert!(text.contains(needle), "{needle}: {text}");
        }
        assert_eq!(
            control.funnel_lockout().get(),
            crate::host_guard::FunnelLockout::Open,
            "a warning, never a lock"
        );
    }

    /// With Tailscale checks off, the start says once what that costs: no
    /// Funnel is noticed, and there is no login. Both serves call this right
    /// after building their loop.
    #[tokio::test]
    async fn a_serve_that_does_not_check_tailscale_says_so_at_start() {
        let (control, _mode_rx) = TailscaleModeControl::new(
            tokio::runtime::Handle::current(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        for (mode, forced_no, says) in [
            (TailscaleMode::No, false, Some("tailscale = \"no\"")),
            (TailscaleMode::Auto, true, Some("--no-tailscale")),
            (TailscaleMode::Yes, true, Some("--no-tailscale")),
            (TailscaleMode::Auto, false, None),
            (TailscaleMode::Yes, false, None),
        ] {
            let ring = dux_core::activity::ActivityRing::new(100);
            let console = Console::capture(ring.clone());
            let ts = TailscaleLoop::new(
                mode,
                forced_no,
                None,
                None,
                &control,
                Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            );
            ts.say_not_checking(&console);
            let texts = ring_texts(&ring);
            match says {
                Some(why) => {
                    assert_eq!(texts.len(), 1, "{mode:?} {forced_no}: {texts:#?}");
                    for needle in ["not checking Tailscale", why, "Funnel", "no login"] {
                        assert!(texts[0].contains(needle), "{needle}: {}", texts[0]);
                    }
                }
                None => assert!(texts.is_empty(), "{mode:?}: {texts:#?}"),
            }
        }
    }

    /// The flip's header URL list follows the serve live: the MagicDNS URL (the
    /// https serve URL when a route points at dux) arrives when the name is
    /// read, moves with a rename, and goes when the mode says `no`.
    #[tokio::test]
    async fn the_flips_url_list_follows_the_name_live() {
        let (_primary, primary_addr) = primary_listener();
        let ring = dux_core::activity::ActivityRing::new(2000);
        let console = Console::capture(ring.clone());
        let (identify, slot) = scripted_identity(named(
            "demo-box.old-tailnet.ts.net",
            &[("https://demo-box.old-tailnet.ts.net", false)],
        ));
        let h =
            Harness::start_with_console(TailscaleMode::Auto, Some(primary_addr), identify, console);
        // The harness registers no legs, so the list is the tailnet extras alone.
        let wait_for = |want: Vec<String>| {
            let ring = ring.clone();
            async move {
                let deadline = tokio::time::Instant::now() + WAIT;
                loop {
                    let got = ring.serve_urls();
                    if got == want {
                        return;
                    }
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "serve URLs are {got:?}, wanted {want:?}"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        };
        wait_for(vec!["https://demo-box.old-tailnet.ts.net".to_string()]).await;
        *slot.lock().unwrap() = Ok(named(
            "demo-box.example-tailnet.ts.net",
            &[("https://demo-box.example-tailnet.ts.net", false)],
        ));
        wait_for(vec!["https://demo-box.example-tailnet.ts.net".to_string()]).await;
        h.control.set_mode(TailscaleMode::No).await;
        wait_for(Vec::new()).await;
        h.finish().await;
    }

    #[tokio::test]
    async fn switching_to_no_forgets_the_name_and_yes_reads_it_again() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, _slot) = scripted_identity(named("box.tail.ts.net", &[]));
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        until_status(&app, "box.tail.ts.net", StatusCode::OK).await;

        h.control.set_mode(TailscaleMode::No).await;
        assert_eq!(
            status_for(&app, "box.tail.ts.net").await,
            StatusCode::FORBIDDEN
        );
        assert!(
            h.control.own_magicdns_name().snapshot().is_empty(),
            "no is a promise to stay off the tailnet, name included"
        );

        // `yes` looks once, the name included.
        assert_eq!(
            h.control.set_mode(TailscaleMode::Yes).await,
            TailscaleModeOutcome::NothingDetected,
            "no address in this test, which is not what this test is about"
        );
        assert_eq!(status_for(&app, "box.tail.ts.net").await, StatusCode::OK);
        h.finish().await;
    }

    #[tokio::test]
    async fn a_serve_route_shows_up_in_the_urls_and_funnel_withdraws_the_name() {
        use axum::http::StatusCode;
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let (_primary, primary_addr) = primary_listener();
        let (identify, slot) = scripted_identity(named(
            "box.tail.ts.net",
            &[("https://box.tail.ts.net", false)],
        ));
        let h = Harness::start_with_identity(
            TailscaleMode::Auto,
            Some(primary_addr),
            Arc::new(|| Err(TailscaleUnavailable::NoAddress)),
            identify,
        );
        let app = router_over(&h, tmp.path());
        until_status(&app, "box.tail.ts.net:443", StatusCode::OK).await;
        let urls = h.control.tailnet_urls(&[primary_addr]);
        assert_eq!(urls.serve, vec!["https://box.tail.ts.net".to_string()]);

        // The same route, now published to the internet.
        *slot.lock().unwrap() = Ok(named(
            "box.tail.ts.net",
            &[("https://box.tail.ts.net", true)],
        ));
        h.control.set_mode(TailscaleMode::Auto).await;
        until_status(&app, "box.tail.ts.net:443", StatusCode::FORBIDDEN).await;
        assert_eq!(
            h.control.tailnet_urls(&[primary_addr]),
            crate::serve_legs::TailnetUrls::default(),
            "no URL is offered on a name the guard refuses"
        );
        h.finish().await;
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
            .expect("the watcher must probe before its first park, not a period later");
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
            "yes starts no ADDRESS watcher, only one for the name"
        );

        // Generation 1: the one the name watcher `yes` starts with is on.
        h.legs
            .send((1, WatchEvent::Leg(LegCommand::Bind(leg))))
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
        h.legs
            .send((1, WatchEvent::Leg(LegCommand::Bind(stale))))
            .await
            .unwrap();
        h.legs
            .send((2, WatchEvent::Leg(LegCommand::Bind(current))))
            .await
            .unwrap();
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
        h.legs
            .send((1, WatchEvent::Leg(LegCommand::Bind(stale))))
            .await
            .unwrap();
        // One lane, so once this has been acted on the stale one has already
        // been decided.
        h.legs
            .send((2, WatchEvent::Leg(LegCommand::Bind(leg))))
            .await
            .unwrap();
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

    #[tokio::test]
    async fn a_mode_that_wants_a_leg_changes_nothing_when_there_is_no_primary() {
        // No primary means no port to hang a leg on, so there is nothing for a
        // watcher to do. Opening the Host guard's tailnet-literal rule anyway
        // would admit tailnet Host headers with nothing watching behind them.
        for mode in [TailscaleMode::Auto, TailscaleMode::Yes] {
            let h = Harness::start(
                TailscaleMode::No,
                false,
                None,
                None,
                Arc::new(|| Ok("127.0.0.2".parse().unwrap())),
            );
            assert_eq!(
                h.control.set_mode(mode).await,
                TailscaleModeOutcome::NoPrimary
            );
            assert!(
                !h.control.host_literals().load(Ordering::SeqCst),
                "{mode:?} must leave the Host guard where it found it"
            );
            assert!(
                !h.control.watched().load(Ordering::SeqCst),
                "{mode:?} must leave nothing claiming to watch"
            );
            h.finish().await;
        }
    }

    #[tokio::test]
    async fn choosing_yes_again_leaves_a_leg_that_is_already_bound_alone() {
        // Re-binding dux's own port fails with EADDRINUSE and warns about a
        // conflict that is dux itself, so the idempotence guard has to hold on
        // the mode the serve is already in.
        let (_primary, primary_addr) = primary_listener();
        let leg = SocketAddr::new(leg_ip(), primary_addr.port());
        // The leg's address genuinely occupied, so a re-bind cannot succeed
        // quietly: it would drop the leg from the registry on the way through.
        let _occupied = std::net::TcpListener::bind(leg).expect("the leg address is free");
        let h = Harness::start(
            TailscaleMode::Yes,
            false,
            Some(primary_addr),
            Some(leg),
            Arc::new(|| Ok(leg_ip())),
        );

        assert_eq!(
            h.control.set_mode(TailscaleMode::Yes).await,
            TailscaleModeOutcome::Applied { bound: Some(leg) }
        );
        assert!(
            h.shutdown.has_leg(leg),
            "the leg that was serving must still be serving"
        );
        assert_eq!(h.bound(), Some(leg));
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
        legs.send((0, WatchEvent::Leg(LegCommand::Bind(leg))))
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
