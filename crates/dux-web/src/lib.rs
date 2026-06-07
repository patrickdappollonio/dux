//! The web layer: exposes the `dux-core` engine over HTTP/WebSocket so a browser
//! SPA can drive the same agent sessions the TUI does.
//!
//! ## Entry points
//!
//! - [`run_server`] — the `dux server` CLI path. Boots the engine on its own
//!   thread, builds the login gate's shared auth snapshot once from config, and
//!   serves axum on a self-built tokio runtime until SIGINT/SIGTERM.
//! - [`serve_with_engine`] — the in-process TUI↔server flip. Serves the web UI
//!   over an EXISTING live engine (PTYs intact) on the caller's thread, returning
//!   the engine when serving stops so the TUI can resume around the same agents.
//!
//! ## Major pieces
//!
//! - [`auth`] — the session-backed login gate: bcrypt verification (in
//!   `dux-core`), the per-IP login backoff, and the [`auth::SharedAuth`] snapshot
//!   that a config reload rebuilds live.
//! - [`server`] — the axum router (open vs gated routes, the gate middleware, the
//!   same-origin WebSocket check) and the `/ws` bridge to the engine.
//! - [`engine_actor`] — the `EngineHandle` and the request/drain loop that owns
//!   the `!Send` engine on its thread, plus the auth-reload hook.
//!
//! ## Dependency isolation
//!
//! This crate depends on `dux-core`, never `dux-tui`. Isolation is enforced by
//! the `dep-isolation` CI job, which runs `cargo tree -p dux-web` and fails if
//! any TUI-only crate appears.

pub mod auth;
pub mod bootstrap;
pub mod console;
pub mod engine_actor;
pub mod protocol;
pub mod server;
pub mod tls;
pub mod web_assets;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use dux_core::config::{DuxPaths, PlanAddr, ServerPlan};
use dux_core::engine::Engine;

use crate::console::{Banner, Console, ListenerRow, LoginRow};
use crate::engine_actor::LoopControl;
use crate::server::RouterParams;
use crate::tls::{AcmePlan, SESSION_SWEEP_PERIOD};

/// Boot the engine on its own thread and serve the web UI on every address in
/// the plan (one axum task per listener, sharing the router/state). Blocking
/// entry — builds its own tokio runtime.
///
/// The auth-reload context's downgrade rule is keyed on `host_only`, computed in
/// [`run_plain_http`] from the addresses that ACTUALLY bound using `is_loopback()`
/// ONLY — NO Tailscale allowance. This is deliberately stricter than the startup
/// bind gate's "local" classification (which treats a Tailscale bind as local). A
/// Tailscale-bound server is reachable by other people's devices on the tailnet,
/// so it is NOT host-only and a running gate must never silently downgrade to open
/// on it. See [`auth::AuthState::rebuild`] for the full distinction.
///
/// `disable_auth` mirrors the `dux server --disable-auth` flag: with it set the
/// login gate is off even when `[auth]` users exist. The gate's shared auth
/// snapshot is built ONCE here from the engine's loaded config users, handed to
/// both the engine actor (so a config reload rebuilds it live) and the router.
///
/// `version` is the dux crate version the binary passes in (`CARGO_PKG_VERSION`)
/// for the console banner header.
///
/// This is the ONLY surface that owns the [`Console`]: it is built here from the
/// engine's loaded `[server] color`/`access_log` and threaded into the serve
/// paths. The TUI flip ([`serve_with_engine`]) NEVER constructs a real console —
/// it keeps its themed status screen and must not print to stdout.
pub fn run_server(
    paths: DuxPaths,
    plan: ServerPlan,
    disable_auth: bool,
    version: String,
) -> Result<()> {
    match plan {
        ServerPlan::PlainHttp { addrs } => run_plain_http(paths, addrs, disable_auth, version),
        ServerPlan::Acme {
            http_addr,
            https_addr,
            domains,
            email,
            production,
            cache_dir,
        } => run_acme(
            paths,
            AcmePlan {
                http_addr,
                https_addr,
                domains,
                email,
                production,
                cache_dir,
            },
            disable_auth,
            version,
        ),
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
/// list and `host_only` are computed from what ACTUALLY bound, not what was
/// requested). `required` is the [`PlanAddr`] tag, retained so the post-bind
/// banner can label a best-effort leg (the LOCAL MODE Tailscale address) as
/// "Tailscale" and a required non-loopback leg as a plain public address.
#[derive(Debug)]
struct BoundListener {
    addr: SocketAddr,
    required: bool,
    listener: tokio::net::TcpListener,
}

/// Bind every [`PlanAddr`], honoring its required/best-effort tag.
///
/// - REQUIRED (loopback, every explicit `listen_addrs` entry): a bind failure is
///   FATAL — it logs a `logger::error` with the failing address and returns the
///   error (with address context) so the serve aborts. This is the
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
/// LAN, or proxy-fronted), sharing the router/state, plus the periodic
/// expired-session sweep. Shutdown is the SAME [`ServeShutdown`] watch lane the
/// ACME path and the flip use: a SIGINT/SIGTERM trips the watch, and the FIRST
/// listener to die records its error and trips the watch too, so the siblings get
/// a graceful shutdown and the error propagates (genuine first-error wind-down —
/// no longer a no-abort JoinSet wait). The single sweep rides the same lane.
///
/// A BEST-EFFORT (Tailscale) address whose bind fails (a third-party process
/// already holds it) does NOT abort the serve: it warns loudly to `dux.log` and
/// the server keeps serving the remaining (bound) addresses. The warning is NOT
/// re-broadcast to web clients — the status broadcast has no replay, and clients
/// only subscribe when their WS connects, which is always after this startup bind,
/// so a startup broadcast would reach zero receivers. `dux.log` and the CLI
/// startup banner (which flags a best-effort leg) are the delivery surfaces for
/// the `dux server` path; the TUI palette flip delivers through its own status
/// line, unchanged. `host_only` is computed from the addresses that ACTUALLY
/// bound, so a dropped Tailscale leg leaves a loopback-only (host-only) server.
/// Build the plain-HTTP startup banner from the BOUND legs (each an
/// `(addr, required)` pair). Each leg is labeled by what it is:
/// - loopback → "Local (loopback)"
/// - a best-effort (LOCAL MODE Tailscale) leg → "Tailscale"
/// - a required non-loopback leg (an explicit `listen_addrs` public/LAN entry) →
///   "Listen"
///
/// The login row is green ("login enabled — N user(s)"), a loud red disabled
/// warning, or — with zero valid users — a "No login required" row whose tone is
/// derived from the legs' reachability (see [`login_row`]/[`reachability`]).
/// Best-effort bind degradations (a busy Tailscale address) become ⚠ rows. Pure
/// (over `(SocketAddr, bool)` pairs, not the live listeners) so it is
/// unit-testable without binding sockets.
fn plain_http_banner(
    version: &str,
    bound: &[(SocketAddr, bool)],
    disable_auth: bool,
    user_count: usize,
    bind_warnings: &[String],
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
                note: None,
            }
        })
        .collect();
    Banner {
        version: version.to_string(),
        mode: "plain HTTP".to_string(),
        login: login_row(disable_auth, reachability(bound), user_count),
        warnings: bind_warnings.to_vec(),
        listeners,
    }
}

/// How far the server can be reached, classified from the BOUND legs (each an
/// `(addr, required)` pair where `required` is true for explicit `listen_addrs`
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
    /// A required non-loopback leg (an explicit `listen_addrs` public/LAN entry)
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

/// The banner's login-state row. `--disable-auth` is a loud red warning; an
/// enabled gate (≥1 valid user) is green with the count.
///
/// Zero valid users means the gate is OFF (per `auth::auth_enabled`) WITHOUT
/// `--disable-auth` — the truthful row is "No login required", never an enabled
/// 0-user row (the old behavior, which lied about a protecting gate). Its tone
/// tracks reachability: a loopback-only bind is the calm local-dev case; a
/// non-loopback leg is a warning, stronger for a public/LAN leg than for a
/// best-effort Tailscale one.
fn login_row(disable_auth: bool, reach: Reachability, user_count: usize) -> LoginRow {
    if disable_auth {
        LoginRow::Disabled
    } else if user_count == 0 {
        match reach {
            Reachability::LoopbackOnly => LoginRow::NoLoginRequired {
                reachable: false,
                public: false,
            },
            Reachability::Tailscale => LoginRow::NoLoginRequired {
                reachable: true,
                public: false,
            },
            Reachability::Public => LoginRow::NoLoginRequired {
                reachable: true,
                public: true,
            },
        }
    } else {
        LoginRow::Enabled { count: user_count }
    }
}

fn run_plain_http(
    paths: DuxPaths,
    addrs: Vec<PlanAddr>,
    disable_auth: bool,
    version: String,
) -> Result<()> {
    let engine = bootstrap::bootstrap_engine(&paths)?;
    let auth = auth::shared_auth(&engine.config.auth.users, disable_auth);
    // Build the vite-style CLI console (color from [server] color) + the access-log
    // toggle, and capture the login-state inputs for the post-bind banner BEFORE
    // the engine moves into the actor thread.
    let (console, access_log) = build_console(&engine.config);
    let user_count = dux_core::auth::parse_users(&engine.config.auth.users).len();
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

        // host-only ⇔ EVERY *bound* listener is genuine loopback. A Tailscale (or
        // public) address that bound makes the server reachable off-host, so the
        // downgrade rule must refuse a live gate-disable there. Computed from what
        // bound so a dropped Tailscale leg correctly leaves a host-only server.
        let host_only = bound.iter().all(|b| b.addr.ip().is_loopback());

        // Post-bind banner: built from what ACTUALLY bound, so it shows truth (no
        // pre-bind hedging). Replaces main.rs's pre-bind URL println. Project the
        // bound listeners into (addr, required) pairs for the pure banner builder.
        let banner_legs: Vec<(SocketAddr, bool)> =
            bound.iter().map(|b| (b.addr, b.required)).collect();
        console.banner(&plain_http_banner(
            &version,
            &banner_legs,
            disable_auth,
            user_count,
            &bind_warnings,
        ));

        // Spawn the engine on its own std thread (it runs the synchronous engine
        // loop, not a tokio task) now that `host_only` is known from the BOUND
        // addresses. The shared auth `Arc` goes to both the actor (live reload
        // refresh) and the router (login/gate reads); the console reaches the
        // reload arm so a live reload echoes on the terminal.
        let (handle, _join) = engine_actor::spawn_engine_thread_with_auth(
            engine,
            engine_actor::AuthReloadContext {
                shared: Arc::clone(&auth),
                disable_auth,
                host_only,
                console: console.clone(),
            },
        );

        // The shared shutdown primitive: a SIGINT/SIGTERM or a first-listener
        // failure flips its watch, every serve task awaits it, and the sweep rides
        // the same lane so it exits with the server rather than lingering.
        let (shutdown, sweep_shutdown_rx) = ServeShutdown::new();
        // Build ONE app + store, clone the router across listeners (it is a cheap
        // `Arc`-backed service). The store is shared (an `Arc`), so the single
        // sweep prunes the same map every listener serves. The console + access-log
        // toggle ride into the router so WS/auth handlers and the access middleware
        // emit to the terminal.
        let (app, store) = server::build_app(
            handle.clone(),
            Arc::clone(&auth),
            axum::Router::new(),
            RouterParams::plain_http().with_console(console.clone(), access_log),
        );
        let sweep = tls::spawn_session_sweep(store, SESSION_SWEEP_PERIOD, sweep_shutdown_rx);

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
                // Serve with connect-info so the login handler can read the peer
                // IP for the per-IP attempt backoff.
                let result = axum::serve(
                    listener,
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
        // Stop the sweep, then SIGTERM the agents (they save state for a later
        // resume), mark their sessions Detached, then exit; Drop hard-kills any
        // straggler.
        shutdown.trigger();
        let _ = sweep.await;
        handle.shutdown().await;
        match shutdown.take_error() {
            Some(e) => Err(e),
            None => Ok::<(), anyhow::Error>(()),
        }
    })
}

/// The loud warning shown when the login gate is OFF on a built-in-TLS server.
///
/// An ACME server is ALWAYS public (a browser-trusted certificate on :443), so a
/// disabled gate means anyone who can reach :443 controls the agents and
/// filesystem. The only safe way to run this is behind an upstream auth proxy
/// (oauth2-proxy and friends). Shared by the `dux server` startup banner (stderr)
/// and the `run_acme` log line (so `dux.log` carries it for long-running servers),
/// so the two can never drift. Pure so it is unit-testable.
pub fn acme_disable_auth_warning() -> String {
    "WARNING: --disable-auth is set and dux is serving built-in TLS on :443 with a \
     browser-trusted certificate but NO login gate. This server is public: anyone who can \
     reach it can control your agents and worktrees. Only do this when an upstream auth proxy \
     (e.g. oauth2-proxy) is handling authentication in front of dux."
        .to_string()
}

/// Build the ACME (built-in TLS) startup banner. The mode line names Let's
/// Encrypt and flags `[STAGING]` when `production` is false; the single listener
/// row is the certificate's primary domain over HTTPS (with the non-default port
/// suffix when `https_port != 443`) plus the `:80` redirect note. An ACME server
/// is always public, so the login row passes `Reachability::Public` — it is the
/// enabled count or the loud `--disable-auth` warning in practice (the resolver
/// refuses ACME without auth-or-explicit-disable, so the public "No login
/// required" branch is unreachable here). Pure so it is unit-testable.
fn acme_banner(
    version: &str,
    domains: &[String],
    https_addr: SocketAddr,
    production: bool,
    disable_auth: bool,
    user_count: usize,
) -> Banner {
    let primary = domains.first().cloned().unwrap_or_default();
    let suffix = if https_addr.port() == 443 {
        String::new()
    } else {
        format!(":{}", https_addr.port())
    };
    let mode = if production {
        "TLS via Let's Encrypt".to_string()
    } else {
        "TLS via Let's Encrypt [STAGING]".to_string()
    };
    let listeners = vec![ListenerRow {
        label: primary.clone(),
        url: format!("https://{primary}{suffix}/"),
        note: Some("(plain HTTP on :80 redirects here & answers ACME challenges)".to_string()),
    }];
    // ACME is always public, so the row is the enabled count, the disabled
    // warning, or — only if it ever reached zero users without --disable-auth,
    // which the ACME resolver forbids — the public "No login required" warning.
    Banner {
        version: version.to_string(),
        mode,
        listeners,
        login: login_row(disable_auth, Reachability::Public, user_count),
        warnings: vec![],
    }
}

/// The ACME (built-in TLS) serve path: two public listeners. `:80` answers the
/// HTTP-01 challenge and otherwise 308-redirects to HTTPS; `:443` serves the
/// existing app router over TLS with the rustls-acme acceptor. A dedicated task
/// polls the `AcmeState` so certificates acquire and renew, and the periodic
/// session sweep runs here too. Shutdown is the SAME [`ServeShutdown`] watch lane
/// `run_plain_http` and the flip use: a SIGINT/SIGTERM or a first-listener failure
/// trips the watch, a watcher awaits it (no sleep-poll) and drives the
/// axum-server `Handle`s' bounded graceful shutdown, so both listeners wind down
/// together and the first error propagates.
///
/// `host_only` is FALSE by nature here (the certs make dux reachable on the
/// public internet), so the live auth-downgrade rule refuses to open the gate.
fn run_acme(paths: DuxPaths, plan: AcmePlan, disable_auth: bool, version: String) -> Result<()> {
    let engine = bootstrap::bootstrap_engine(&paths)?;
    // Mirror the startup banner into dux.log so a long-running TLS server that was
    // launched with the gate off keeps a visible record of it — the banner only
    // appears once on stderr at boot.
    if disable_auth {
        dux_core::logger::warn(&format!("[server] {}", acme_disable_auth_warning()));
    }
    let auth = auth::shared_auth(&engine.config.auth.users, disable_auth);
    // Build the CLI console + access-log toggle and capture the banner inputs
    // BEFORE the engine moves into the actor thread.
    let (console, access_log) = build_console(&engine.config);
    let user_count = dux_core::auth::parse_users(&engine.config.auth.users).len();
    let production = plan.production;
    let https_addr = plan.https_addr;
    let (handle, _join) = engine_actor::spawn_engine_thread_with_auth(
        engine,
        engine_actor::AuthReloadContext {
            shared: Arc::clone(&auth),
            disable_auth,
            // ACME serving is public by nature: never host-only, so a live
            // reload that removes the last user must NOT silently open the gate.
            host_only: false,
            console: console.clone(),
        },
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let https_port = plan.https_addr.port();

    runtime.block_on(async move {
        // Build the ACME state (creates the 0700 cache dir, normalizes domains)
        // and the normalized domain list reused for the Host allowlist + the
        // :80 redirect router.
        let (acme_state, domains) = tls::build_acme_state(&plan)?;

        // Post-bind banner: the normalized domains are now known, so the banner
        // shows the certificate's primary hostname + the :80 redirect note.
        // ACME serving is always public, so the gate state uses host_only = false.
        console.banner(&acme_banner(
            &version,
            &domains,
            https_addr,
            production,
            disable_auth,
            user_count,
        ));

        // The :80 challenge service borrows the SAME state's resolver, so build
        // the challenge router BEFORE moving the state into the polling task.
        let http_router = tls::build_http_challenge_router(
            &acme_state,
            https_port,
            domains.clone(),
            console.clone(),
            access_log,
        );
        let acceptor = acme_state.axum_acceptor(acme_state.default_rustls_config());

        // Drive certificate acquisition/renewal. Without this nothing progresses.
        // Thread the engine's status broadcast in so certificate lifecycle events
        // (acquired/renewed, errors, stream-end) reach web clients live — the same
        // channel the reload warnings ride — not just dux.log — AND the CLI console.
        let acme_task = tls::spawn_acme_event_task(
            acme_state,
            Some(handle.status_sender()),
            console.clone(),
            domains.clone(),
        );

        // Build the HTTPS app (Secure cookie ON) + its session store, then pin
        // every route to the configured domains (DNS-rebinding defense). The
        // console + access-log toggle ride into the router too.
        let (https_app, store) = server::build_app(
            handle.clone(),
            Arc::clone(&auth),
            axum::Router::new(),
            RouterParams::tls().with_console(console.clone(), access_log),
        );
        // Pin every route to the configured domains (DNS-rebinding defense) and
        // stamp HSTS on every response. Both are HTTPS-ONLY hardening: dux owns
        // TLS on this path, so it is correct to tell the browser this host is
        // HTTPS-only. The plain-HTTP/proxy/flip paths get neither.
        let https_app = tls::host_allowlist_layer(https_app, domains.clone());
        let https_app = tls::hsts_layer(https_app);

        // The shared shutdown primitive: a SIGINT/SIGTERM or a first-listener
        // failure flips its watch; the watcher below awaits it and drives the
        // axum-server handles' bounded graceful shutdown. The sweep rides the same
        // lane so it exits with the server.
        let (shutdown, sweep_shutdown_rx) = ServeShutdown::new();
        let sweep = tls::spawn_session_sweep(store, SESSION_SWEEP_PERIOD, sweep_shutdown_rx);

        // axum-server graceful-shutdown handles, one per listener, both driven by
        // the shared watch below.
        let http_handle = axum_server::Handle::new();
        let https_handle = axum_server::Handle::new();

        let mut tasks = tokio::task::JoinSet::new();
        {
            let http_addr = plan.http_addr;
            let h = http_handle.clone();
            let shutdown = shutdown.clone();
            tasks.spawn(async move {
                let r = tls::serve_http_challenge(http_addr, http_router, h).await;
                if let Err(e) = &r {
                    dux_core::logger::error(&format!(
                        "[server] the ACME challenge/redirect listener on {http_addr} failed: {e} \
                         — is something already listening there?"
                    ));
                    shutdown.record_failure(anyhow::anyhow!(
                        "the ACME challenge/redirect listener on {http_addr} failed: {e}"
                    ));
                }
                r
            });
        }
        {
            let https_addr = plan.https_addr;
            let h = https_handle.clone();
            let shutdown = shutdown.clone();
            tasks.spawn(async move {
                let r = tls::serve_https_acme(https_addr, https_app, acceptor, h).await;
                if let Err(e) = &r {
                    dux_core::logger::error(&format!(
                        "[server] the TLS listener on {https_addr} failed: {e} \
                         — is something already listening there?"
                    ));
                    shutdown.record_failure(anyhow::anyhow!(
                        "the TLS listener on {https_addr} failed: {e}"
                    ));
                }
                r
            });
        }

        // Boot log for the pre-first-cert window. rustls-acme has not issued a
        // certificate yet at this point, so TLS handshakes on :443 will FAIL until
        // the first cert arrives from Let's Encrypt (driven by the poller task and
        // the :80 HTTP-01 challenge). Say so loudly per the explicit-failure tenet
        // so an operator watching the log knows the early handshake failures are
        // expected, not a misconfiguration.
        let directory = if plan.production {
            "production"
        } else {
            "staging"
        };
        dux_core::logger::info(&format!(
            "[server] TLS listener up on {} — waiting for the first certificate from {directory} \
             Let's Encrypt (HTTP-01 challenge served on {}). HTTPS handshakes will FAIL until that \
             certificate is issued; watch this log for the [acme] certificate lifecycle events.",
            plan.https_addr, plan.http_addr
        ));

        // Signal/abort watcher: a SIGINT/SIGTERM OR a first listener error both
        // flip the shared watch; this awaits it (no sleep-poll) and drives the
        // bounded graceful shutdown on both axum-server handles. A separate task
        // translates the OS signal into a watch trip so the two triggers converge
        // on the same lane.
        {
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                shutdown_signal().await;
                shutdown.trigger();
            });
        }
        {
            let http_handle = http_handle.clone();
            let https_handle = https_handle.clone();
            let watch_rx = shutdown.subscribe();
            tokio::spawn(async move {
                wait_for_shutdown(watch_rx).await;
                // Bounded graceful shutdown so a wedged TLS client can't hang exit.
                http_handle.graceful_shutdown(Some(ACME_GRACEFUL_SHUTDOWN));
                https_handle.graceful_shutdown(Some(ACME_GRACEFUL_SHUTDOWN));
            });
        }

        // Wait for both serve tasks to finish. A serve task that returned `Err`
        // already recorded itself via `shutdown.record_failure` inside the task;
        // but a task that PANICKED yields a `JoinError` here and recorded nothing,
        // so record the JoinError too and trip the watch so the sibling listener
        // winds down — consistent with `run_plain_http`.
        while let Some(joined) = tasks.join_next().await {
            if let Err(join_err) = joined {
                dux_core::logger::error(&format!(
                    "[server] an ACME serve task panicked: {join_err} — shutting the other \
                     listener down so the server does not limp on half-dead."
                ));
                shutdown.record_failure(anyhow::anyhow!("an ACME serve task panicked: {join_err}"));
            }
        }

        // Wind down the ACME poller and the sweep (the watch is already tripped,
        // so the sweep is winding down; trip again is idempotent).
        acme_task.abort();
        shutdown.trigger();
        let _ = sweep.await;

        handle.shutdown().await;

        match shutdown.take_error() {
            Some(e) => Err(e),
            None => Ok::<(), anyhow::Error>(()),
        }
    })
}

/// Bounded graceful-shutdown window for the ACME listeners: long enough for
/// in-flight requests to finish, short enough that a wedged TLS connection cannot
/// hang process exit.
const ACME_GRACEFUL_SHUTDOWN: Duration = Duration::from_secs(3);

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
/// (`run_plain_http`, `run_acme`, `serve_with_engine`). It bundles the
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
///   ACME `AtomicBool` + 100ms poll AND `run_plain_http`'s no-abort JoinSet wait.
#[derive(Clone)]
struct ServeShutdown {
    failed: Arc<AtomicBool>,
    error: Arc<std::sync::Mutex<Option<anyhow::Error>>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl ServeShutdown {
    fn new() -> (Self, tokio::sync::watch::Receiver<bool>) {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        (
            Self {
                failed: Arc::new(AtomicBool::new(false)),
                error: Arc::new(std::sync::Mutex::new(None)),
                shutdown_tx,
            },
            shutdown_rx,
        )
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
    /// (graceful shutdown returns `Ok`, so this only fires on a genuine death) and
    /// by the ACME join-error handler. The FIRST caller wins: it stores the error
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
/// so each caller passes its own [`ServeShutdown::subscribe`] handle. Replaces the
/// ACME path's `wait_for_flag` 100ms sleep-poll with a wakeup-driven await.
async fn wait_for_shutdown(mut rx: tokio::sync::watch::Receiver<bool>) {
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            break;
        }
    }
}

/// Classify a set of live std listeners for the auth-reload DOWNGRADE rule.
///
/// Returns `true` (host-only) only when EVERY listener's local address is
/// genuine loopback. A Tailscale (or any non-loopback) listener makes the server
/// reachable off-host, so it returns `false` and the live gate cannot silently
/// downgrade to open there. This is intentionally STRICTER than the startup bind
/// gate's "local" classification, which treats a Tailscale bind as local — see
/// [`auth::AuthState::rebuild`]. A listener whose local address cannot be read is
/// treated as NOT loopback (fail closed: never accidentally classify an unknown
/// listener as host-only).
fn resolve_host_only(listeners: &[std::net::TcpListener]) -> bool {
    listeners.iter().all(|l| {
        l.local_addr()
            .map(|a| a.ip().is_loopback())
            .unwrap_or(false)
    })
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
    mut on_tick: impl FnMut() -> ServerTick,
) -> Result<(Engine, ServerExit)> {
    // The flip never disables auth (there is no TUI `--disable-auth` path), and
    // the flip preset's engine config typically has no `[auth]` users, so the
    // gate is off and the UX is unchanged. Building the snapshot from the live
    // engine config still means a user added to config + reload-config turns the
    // gate on mid-flip. The same `Arc` is threaded into both the actor (live
    // reload) and the router.
    let auth = auth::shared_auth(&engine.config.auth.users, false);
    // The downgrade rule is keyed on `host_only`, computed from the ACTUAL
    // listeners: TRUE only when EVERY listener is genuine loopback. The flip's
    // `local_addrs` may include the machine's Tailscale address — and although
    // the startup bind gate treats that as "local", the downgrade rule must NOT:
    // a Tailscale bind is reachable by other devices on the shared tailnet, so a
    // running gate must never silently open there. A loopback-only flip stays
    // host-only (a reload that removes the last user is allowed); a flip that
    // bound the Tailscale address flips host_only false (such a reload is
    // refused). `resolve_host_only` centralises the per-listener classification.
    let host_only = resolve_host_only(&listeners);
    let (handle, ends) = engine_actor::build_actor_channels_with_auth(
        &engine,
        Some(engine_actor::AuthReloadContext {
            shared: Arc::clone(&auth),
            disable_auth: false,
            host_only,
            // The flip owns the terminal with its themed status screen, so the
            // reload arm must NOT print to stdout — a no-op console.
            console: Console::noop(),
        }),
    );
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
    let (shutdown, sweep_shutdown_rx) = ServeShutdown::new();
    // Set by the signal task; polled by the control closure so a SIGINT/SIGTERM
    // received while serving breaks the engine loop too (not just axum). Distinct
    // from the failure flag because a signal means QuitProcess, a failure means
    // ReturnToTui-with-error.
    let signal_quit = Arc::new(AtomicBool::new(false));

    // Build ONE app + session store, shared across listeners (the router is a
    // cheap `Arc`-backed service; the store is an `Arc`). The flip is plain HTTP
    // by type, so no Secure cookie. The periodic expired-session sweep prunes the
    // shared store and stops when the shutdown watch flips on teardown.
    let (app, sweep_store) = server::build_app(
        handle.clone(),
        Arc::clone(&auth),
        axum::Router::new(),
        RouterParams::plain_http(),
    );
    let sweep_task = {
        let _guard = runtime.enter();
        tls::spawn_session_sweep(sweep_store, SESSION_SWEEP_PERIOD, sweep_shutdown_rx)
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
                let result = axum::serve(
                    tokio_listener,
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
    // connection on any listener from hanging the flip back to the TUI. The same
    // watch flip also stops the session sweep task; await it (bounded) too.
    shutdown.trigger();
    runtime.block_on(async {
        let _ = tokio::time::timeout(SERVER_JOIN_TIMEOUT, async {
            while server_tasks.join_next().await.is_some() {}
        })
        .await;
        let _ = tokio::time::timeout(SERVER_JOIN_TIMEOUT, sweep_task).await;
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

    if matches!(exit, ServerExit::ReturnToTui) {
        // Restore default SIGINT/SIGTERM dispositions before handing control back
        // to the TUI. tokio's unix signal support registers process-global
        // handlers via signal-hook-registry that are NOT removed when the runtime
        // is torn down: after one flip the dispositions stay non-default, so the
        // resumed TUI would no longer die to an external `kill`/`kill -INT`.
        // Resetting to SIG_DFL restores normal terminate-on-signal behavior.
        // QuitProcess doesn't need this — the caller exits the process anyway.
        //
        // Unix-only by project policy (CLAUDE.md targets macOS + Linux), so no
        // cfg gating is needed.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
        }
    }

    // If a listener's accept loop died (F5), surface the captured error rather
    // than reporting a clean exit. The engine has already been wound down above,
    // so the caller drops it; the TUI shows the failure instead of resuming onto
    // a server that silently stopped serving.
    if let Some(err) = shutdown.take_error() {
        return Err(err);
    }

    Ok((engine, exit))
}

/// Resolves when the process receives SIGINT (Ctrl-C) or SIGTERM, the standard
/// signals an operator or supervisor sends to stop the server.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Reachability, ServeShutdown, acme_banner, acme_disable_auth_warning, bind_plan_addrs,
        login_row, plain_http_banner, reachability, resolve_host_only, tailscale_bind_warning,
        wait_for_shutdown,
    };
    use crate::console::LoginRow;
    use dux_core::config::PlanAddr;
    use dux_core::engine::Command;

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

    #[test]
    fn acme_disable_auth_warning_flags_public_no_gate() {
        // The warning the banner prints and run_acme logs must name the risk
        // (public, no gate) and the only safe mitigation (an upstream auth proxy).
        let w = acme_disable_auth_warning();
        assert!(w.contains("--disable-auth"), "must name the flag: {w}");
        assert!(w.contains("NO login gate"), "must say the gate is off: {w}");
        assert!(
            w.to_lowercase().contains("public"),
            "must call the server public: {w}"
        );
        assert!(
            w.contains("auth proxy"),
            "must point at the upstream-proxy mitigation: {w}"
        );
    }

    // ── Startup banner builders ────────────────────────────────────────────

    fn addr(s: &str) -> std::net::SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn login_row_disabled_when_auth_flag_set() {
        // --disable-auth wins regardless of reachability or user count.
        assert!(matches!(
            login_row(true, Reachability::LoopbackOnly, 0),
            LoginRow::Disabled
        ));
        assert!(matches!(
            login_row(true, Reachability::Public, 3),
            LoginRow::Disabled
        ));
    }

    #[test]
    fn login_row_enabled_with_count() {
        // ≥1 valid user is the green enabled row, reachability irrelevant.
        match login_row(false, Reachability::Public, 2) {
            LoginRow::Enabled { count } => assert_eq!(count, 2),
            other => panic!("expected enabled, got {other:?}"),
        }
        match login_row(false, Reachability::LoopbackOnly, 1) {
            LoginRow::Enabled { count } => assert_eq!(count, 1),
            other => panic!("expected enabled, got {other:?}"),
        }
    }

    #[test]
    fn login_row_zero_users_loopback_is_no_login_required_muted() {
        // Zero users on a loopback-only bind: the gate is OFF, so the honest row
        // is the calm muted "No login required (local only)" — never an enabled
        // 0-user row (which would lie about a protecting gate).
        assert!(matches!(
            login_row(false, Reachability::LoopbackOnly, 0),
            LoginRow::NoLoginRequired {
                reachable: false,
                public: false,
            }
        ));
    }

    #[test]
    fn login_row_zero_users_tailscale_is_no_login_required_warning() {
        // A best-effort Tailscale leg is reachable off-host → warning, but not the
        // stronger public wording (public: false).
        assert!(matches!(
            login_row(false, Reachability::Tailscale, 0),
            LoginRow::NoLoginRequired {
                reachable: true,
                public: false,
            }
        ));
    }

    #[test]
    fn login_row_zero_users_public_is_no_login_required_public_warning() {
        // A required public/LAN leg is reachable → the strongest "No login
        // required" warning (public: true).
        assert!(matches!(
            login_row(false, Reachability::Public, 0),
            LoginRow::NoLoginRequired {
                reachable: true,
                public: true,
            }
        ));
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

    #[test]
    fn plain_http_banner_labels_loopback_tailscale_and_public_legs() {
        let legs = vec![
            (addr("127.0.0.1:8080"), true),   // loopback (required)
            (addr("100.64.0.5:8080"), false), // best-effort → Tailscale
            (addr("203.0.113.7:8080"), true), // required non-loopback → Listen
        ];
        let banner = plain_http_banner("0.1.0", &legs, false, 1, &[]);
        assert_eq!(banner.mode, "plain HTTP");
        assert_eq!(banner.listeners.len(), 3);
        assert_eq!(banner.listeners[0].label, "Local (loopback)");
        assert_eq!(banner.listeners[0].url, "http://127.0.0.1:8080");
        assert_eq!(banner.listeners[1].label, "Tailscale");
        assert_eq!(banner.listeners[2].label, "Listen");
        assert!(matches!(banner.login, LoginRow::Enabled { count: 1 }));
    }

    #[test]
    fn plain_http_banner_carries_degradation_warnings() {
        let legs = vec![(addr("127.0.0.1:8080"), true)];
        let warnings = vec!["Tailscale: 100.64.0.1:8080 busy — serving without it".to_string()];
        let banner = plain_http_banner("0.1.0", &legs, false, 0, &warnings);
        assert_eq!(banner.warnings, warnings);
        // Zero users on a loopback-only bind → the muted "No login required (local
        // only)" row, never an enabled 0-user row and never disabled.
        assert!(matches!(
            banner.login,
            LoginRow::NoLoginRequired {
                reachable: false,
                public: false,
            }
        ));
    }

    #[test]
    fn plain_http_banner_zero_users_public_leg_is_public_warning() {
        // A required public/LAN leg with zero users → the strongest "No login
        // required" warning, derived from the bound legs.
        let legs = vec![(addr("127.0.0.1:8080"), true), (addr("0.0.0.0:8080"), true)];
        let banner = plain_http_banner("0.1.0", &legs, false, 0, &[]);
        assert!(matches!(
            banner.login,
            LoginRow::NoLoginRequired {
                reachable: true,
                public: true,
            }
        ));
    }

    #[test]
    fn plain_http_banner_zero_users_tailscale_leg_is_tailscale_warning() {
        // A best-effort Tailscale leg with zero users → the warning row, but not
        // the stronger public wording.
        let legs = vec![
            (addr("127.0.0.1:8080"), true),
            (addr("100.64.0.5:8080"), false),
        ];
        let banner = plain_http_banner("0.1.0", &legs, false, 0, &[]);
        assert!(matches!(
            banner.login,
            LoginRow::NoLoginRequired {
                reachable: true,
                public: false,
            }
        ));
    }

    #[test]
    fn plain_http_banner_disabled_auth_row() {
        let legs = vec![(addr("0.0.0.0:8080"), true)];
        let banner = plain_http_banner("0.1.0", &legs, true, 0, &[]);
        assert!(matches!(banner.login, LoginRow::Disabled));
    }

    #[test]
    fn acme_banner_production_mode_and_redirect_note() {
        let domains = vec!["dux.example.com".to_string()];
        let banner = acme_banner("0.1.0", &domains, addr("0.0.0.0:443"), true, false, 2);
        assert_eq!(banner.mode, "TLS via Let's Encrypt");
        assert_eq!(banner.listeners.len(), 1);
        assert_eq!(banner.listeners[0].label, "dux.example.com");
        assert_eq!(banner.listeners[0].url, "https://dux.example.com/");
        assert!(
            banner.listeners[0]
                .note
                .as_deref()
                .is_some_and(|n| n.contains("redirects here"))
        );
        assert!(matches!(banner.login, LoginRow::Enabled { count: 2 }));
    }

    #[test]
    fn acme_banner_staging_mode_and_nondefault_port() {
        let domains = vec!["dux.example.com".to_string()];
        let banner = acme_banner("0.1.0", &domains, addr("0.0.0.0:8443"), false, false, 1);
        assert_eq!(banner.mode, "TLS via Let's Encrypt [STAGING]");
        assert_eq!(banner.listeners[0].url, "https://dux.example.com:8443/");
    }

    #[test]
    fn acme_banner_disabled_auth_row() {
        let domains = vec!["dux.example.com".to_string()];
        // ACME is always public (host_only=false), so disable_auth → loud row.
        let banner = acme_banner("0.1.0", &domains, addr("0.0.0.0:443"), true, true, 0);
        assert!(matches!(banner.login, LoginRow::Disabled));
    }

    #[test]
    fn record_serve_failure_first_caller_wins_and_triggers_shutdown() {
        // The first serve task to die records its error, arms the flag, and trips
        // the shutdown watch; a later caller (another listener winding down) does
        // NOT overwrite the first error but STILL nudges shutdown. This is the F5
        // load-bearing logic, tested directly because forcing a real axum accept
        // loop to error mid-serve is inherently flaky. Now exercised through the
        // ONE shared [`ServeShutdown`] primitive every serve path uses.
        let (shutdown, mut shutdown_rx) = ServeShutdown::new();

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
        let (shutdown, _rx) = ServeShutdown::new();
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
        let (shutdown, _rx) = ServeShutdown::new();
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
    fn resolve_host_only_true_for_loopback_only_listeners() {
        // Two loopback listeners → host-only (a loopback-only flip permits the
        // documented downgrade on reload).
        let a = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback v4");
        let b = std::net::TcpListener::bind("[::1]:0").expect("bind loopback v6");
        assert!(
            resolve_host_only(&[a, b]),
            "loopback-only listeners must classify as host-only"
        );
    }

    #[test]
    fn resolve_host_only_false_when_a_non_loopback_listener_is_present() {
        // A loopback listener PLUS a non-loopback (wildcard `0.0.0.0`, standing in
        // for a Tailscale/public bind we can't allocate in CI) → NOT host-only,
        // so the live gate downgrade is refused. This is the F1 guard at the
        // resolution layer: any reachable listener flips host_only false even when
        // a loopback listener is also present.
        let loopback = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let wildcard = std::net::TcpListener::bind("0.0.0.0:0").expect("bind wildcard");
        assert!(
            !resolve_host_only(&[loopback, wildcard]),
            "a non-loopback listener must flip host_only false"
        );
        // And a sole non-loopback listener is likewise not host-only.
        let only_wildcard = std::net::TcpListener::bind("0.0.0.0:0").expect("bind wildcard");
        assert!(!resolve_host_only(&[only_wildcard]));
    }

    #[test]
    fn resolve_host_only_empty_is_vacuously_true() {
        // No listeners: `all` is vacuously true. Not a real runtime case (the flip
        // always binds at least loopback), but pins the boundary.
        assert!(resolve_host_only(&[]));
    }

    /// Light smoke test that the public dux-core API can be invoked from
    /// dux-web without TUI imports. Real architectural enforcement of the
    /// "no TUI deps" rule lives in the `dep-isolation` CI job.
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
mod config_saver_tests {
    use dux_core::config::{Config, DuxPaths};
    use dux_core::engine::ConfigSaver;
    use dux_core::worker::WorkerEvent;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Sender};

    /// Web-layer placeholder: today no-op. Sub-project #3 will replace this
    /// with whatever persistence semantics the web layer actually needs.
    struct WebConfigSaver;

    impl ConfigSaver for WebConfigSaver {
        fn persist_global_env(
            &self,
            env: BTreeMap<String, String>,
            _config: Config,
            _config_path: PathBuf,
            worker_tx: Sender<WorkerEvent>,
        ) {
            let _ = worker_tx.send(WorkerEvent::GlobalEnvPersistenceCompleted {
                env,
                result: Ok(()),
            });
        }

        fn persist_macros(
            &self,
            config: Config,
            _config_path: PathBuf,
            worker_tx: Sender<WorkerEvent>,
        ) {
            let _ = worker_tx.send(WorkerEvent::MacrosPersistenceCompleted {
                macros: config.macros,
                result: Ok(()),
            });
        }

        fn reload_config(&self, _paths: DuxPaths, worker_tx: Sender<WorkerEvent>) {
            let _ = worker_tx.send(WorkerEvent::ConfigReloadReady(Box::new(Ok(
                Config::default(),
            ))));
        }

        fn recover_config(
            &self,
            _config_path: PathBuf,
            _config: Config,
            worker_tx: Sender<WorkerEvent>,
        ) {
            let _ = worker_tx.send(WorkerEvent::ConfigRecoverCompleted(Ok(())));
        }
    }

    /// Proves the web layer can implement `ConfigSaver` against `dux-core`
    /// alone (no TUI deps). This is what unblocks sub-project #3 from
    /// reusing the engine's config-persistence command dispatch.
    #[test]
    fn web_can_implement_config_saver() {
        let (tx, rx) = mpsc::channel();
        let saver: Box<dyn ConfigSaver> = Box::new(WebConfigSaver);
        let mut env = BTreeMap::new();
        env.insert("X".into(), "1".into());
        saver.persist_global_env(
            env,
            Config::default(),
            PathBuf::from("/tmp/dux-web-test"),
            tx,
        );
        let event = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("event");
        assert!(matches!(
            event,
            WorkerEvent::GlobalEnvPersistenceCompleted { result: Ok(()), .. }
        ));
    }
}
