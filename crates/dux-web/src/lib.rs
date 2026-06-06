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
use dux_core::config::{DuxPaths, ServerPlan};
use dux_core::engine::Engine;

use crate::engine_actor::LoopControl;
use crate::server::RouterParams;
use crate::tls::{AcmePlan, SESSION_SWEEP_PERIOD};

/// Boot the engine on its own thread and serve the web UI on every address in
/// `addrs` (one axum task per listener, sharing the router/state). Blocking
/// entry — builds its own tokio runtime.
///
/// The auth-reload context's downgrade rule is keyed on `host_only`, computed
/// here from the actual `addrs` using `is_loopback()` ONLY — NO Tailscale
/// allowance. This is deliberately stricter than the startup bind gate's "local"
/// classification (which treats a Tailscale bind as local). A Tailscale-bound
/// server is reachable by other people's devices on the tailnet, so it is NOT
/// host-only and a running gate must never silently downgrade to open on it. See
/// [`auth::AuthState::rebuild`] for the full distinction.
///
/// `disable_auth` mirrors the `dux server --disable-auth` flag: with it set the
/// login gate is off even when `[auth]` users exist. The gate's shared auth
/// snapshot is built ONCE here from the engine's loaded config users, handed to
/// both the engine actor (so a config reload rebuilds it live) and the router.
pub fn run_server(paths: DuxPaths, plan: ServerPlan, disable_auth: bool) -> Result<()> {
    match plan {
        ServerPlan::PlainHttp { addrs } => run_plain_http(paths, addrs, disable_auth),
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
        ),
    }
}

/// The plain-HTTP serve path: one axum task per listener (loopback, Tailscale,
/// LAN, or proxy-fronted), sharing the router/state, plus the periodic
/// expired-session sweep. Behaviorally identical to the pre-TLS `run_server`
/// except for the added sweep task (deferral 3).
fn run_plain_http(paths: DuxPaths, addrs: Vec<SocketAddr>, disable_auth: bool) -> Result<()> {
    let engine = bootstrap::bootstrap_engine(&paths)?;
    // host-only ⇔ EVERY listener is genuine loopback. A Tailscale (or public)
    // address makes the server reachable off-host, so the downgrade rule must
    // refuse a live gate-disable there.
    let host_only = addrs.iter().all(|a| a.ip().is_loopback());
    // Build the login gate's shared state from the loaded config users (parsed
    // ONCE here, not per request). The same `Arc` is handed to the engine actor
    // (for live reload refresh) and to the router (for login/gate reads).
    let auth = auth::shared_auth(&engine.config.auth.users, disable_auth);
    let (handle, _join) = engine_actor::spawn_engine_thread_with_auth(
        engine,
        engine_actor::AuthReloadContext {
            shared: Arc::clone(&auth),
            disable_auth,
            host_only,
        },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        // Drives the session sweep's lifetime: flipped to `true` once all serve
        // tasks finish (a shutdown signal fired), so the sweep task exits with the
        // server rather than lingering.
        let (sweep_shutdown_tx, sweep_shutdown_rx) = tokio::sync::watch::channel(false);
        // Build ONE app + store, clone the router across listeners (it is a cheap
        // `Arc`-backed service). The store is shared (an `Arc`), so the single
        // sweep prunes the same map every listener serves.
        let (app, store) = server::build_app(
            handle.clone(),
            Arc::clone(&auth),
            axum::Router::new(),
            RouterParams::plain_http(),
        );
        let sweep = tls::spawn_session_sweep(store, SESSION_SWEEP_PERIOD, sweep_shutdown_rx);

        // Bind every address; each serve task gets its own graceful-shutdown
        // future driven by the same signal.
        let mut tasks = tokio::task::JoinSet::new();
        for addr in addrs {
            let listener = tokio::net::TcpListener::bind(addr).await?;
            let app = app.clone();
            tasks.spawn(async move {
                // Serve with connect-info so the login handler can read the peer
                // IP for the per-IP attempt backoff.
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(shutdown_signal())
                .await
            });
        }
        // Wait for all serve tasks to finish (they all wind down together when a
        // shutdown signal fires). Surface the first error if any task failed.
        let mut first_err: Option<anyhow::Error> = None;
        while let Some(joined) = tasks.join_next().await {
            if let Err(e) = joined
                .map_err(anyhow::Error::from)
                .and_then(|r| r.map_err(Into::into))
                && first_err.is_none()
            {
                first_err = Some(e);
            }
        }
        // Stop the sweep, then SIGTERM the agents (they save state for a later
        // resume), mark their sessions Detached, then exit; Drop hard-kills any
        // straggler.
        let _ = sweep_shutdown_tx.send(true);
        let _ = sweep.await;
        handle.shutdown().await;
        match first_err {
            Some(e) => Err(e),
            None => Ok::<(), anyhow::Error>(()),
        }
    })
}

/// The ACME (built-in TLS) serve path: two public listeners. `:80` answers the
/// HTTP-01 challenge and otherwise 308-redirects to HTTPS; `:443` serves the
/// existing app router over TLS with the rustls-acme acceptor. A dedicated task
/// polls the `AcmeState` so certificates acquire and renew, and the periodic
/// session sweep runs here too. Graceful shutdown is wired through axum-server
/// `Handle`s on the same signal lane, and the FIRST listener error aborts both,
/// mirroring `run_plain_http`/`serve_with_engine`.
///
/// `host_only` is FALSE by nature here (the certs make dux reachable on the
/// public internet), so the live auth-downgrade rule refuses to open the gate.
fn run_acme(paths: DuxPaths, plan: AcmePlan, disable_auth: bool) -> Result<()> {
    let engine = bootstrap::bootstrap_engine(&paths)?;
    let auth = auth::shared_auth(&engine.config.auth.users, disable_auth);
    let (handle, _join) = engine_actor::spawn_engine_thread_with_auth(
        engine,
        engine_actor::AuthReloadContext {
            shared: Arc::clone(&auth),
            disable_auth,
            // ACME serving is public by nature: never host-only, so a live
            // reload that removes the last user must NOT silently open the gate.
            host_only: false,
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

        // The :80 challenge service borrows the SAME state's resolver, so build
        // the challenge router BEFORE moving the state into the polling task.
        let http_router =
            tls::build_http_challenge_router(&acme_state, https_port, domains.clone());
        let acceptor = acme_state.axum_acceptor(acme_state.default_rustls_config());

        // Drive certificate acquisition/renewal. Without this nothing progresses.
        let acme_task = tls::spawn_acme_event_task(acme_state);

        // Build the HTTPS app (Secure cookie ON) + its session store, then pin
        // every route to the configured domains (DNS-rebinding defense).
        let (https_app, store) = server::build_app(
            handle.clone(),
            Arc::clone(&auth),
            axum::Router::new(),
            RouterParams::tls(),
        );
        let https_app = tls::host_allowlist_layer(https_app, domains.clone());

        // Sweep lifetime + shutdown lane.
        let (sweep_shutdown_tx, sweep_shutdown_rx) = tokio::sync::watch::channel(false);
        let sweep = tls::spawn_session_sweep(store, SESSION_SWEEP_PERIOD, sweep_shutdown_rx);

        // axum-server graceful-shutdown handles, one per listener, both driven by
        // the shared signal below.
        let http_handle = axum_server::Handle::new();
        let https_handle = axum_server::Handle::new();

        // First-error abort across BOTH listeners (mirrors run_plain_http /
        // serve_with_engine): the first serve task to die trips this flag so the
        // signal task can wind the other listener down too.
        let serve_failed = Arc::new(AtomicBool::new(false));
        let serve_error: Arc<std::sync::Mutex<Option<anyhow::Error>>> =
            Arc::new(std::sync::Mutex::new(None));

        let mut tasks = tokio::task::JoinSet::new();
        {
            let http_addr = plan.http_addr;
            let h = http_handle.clone();
            let failed = Arc::clone(&serve_failed);
            let errslot = Arc::clone(&serve_error);
            tasks.spawn(async move {
                let r = tls::serve_http_challenge(http_addr, http_router, h).await;
                if let Err(e) = &r {
                    dux_core::logger::error(&format!(
                        "[server] the :80 ACME challenge/redirect listener failed: {e}"
                    ));
                    record_serve_failure_named(
                        &failed,
                        &errslot,
                        anyhow::anyhow!("the :80 ACME challenge/redirect listener failed: {e}"),
                    );
                }
                r
            });
        }
        {
            let https_addr = plan.https_addr;
            let h = https_handle.clone();
            let failed = Arc::clone(&serve_failed);
            let errslot = Arc::clone(&serve_error);
            tasks.spawn(async move {
                let r = tls::serve_https_acme(https_addr, https_app, acceptor, h).await;
                if let Err(e) = &r {
                    dux_core::logger::error(&format!("[server] the :443 TLS listener failed: {e}"));
                    record_serve_failure_named(
                        &failed,
                        &errslot,
                        anyhow::anyhow!("the :443 TLS listener failed: {e}"),
                    );
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

        // Signal/abort watcher: a SIGINT/SIGTERM OR a first listener error
        // triggers graceful shutdown on both axum-server handles.
        {
            let http_handle = http_handle.clone();
            let https_handle = https_handle.clone();
            let serve_failed = Arc::clone(&serve_failed);
            tokio::spawn(async move {
                tokio::select! {
                    _ = shutdown_signal() => {}
                    _ = wait_for_flag(serve_failed) => {}
                }
                // Bounded graceful shutdown so a wedged TLS client can't hang exit.
                http_handle.graceful_shutdown(Some(ACME_GRACEFUL_SHUTDOWN));
                https_handle.graceful_shutdown(Some(ACME_GRACEFUL_SHUTDOWN));
            });
        }

        // Wait for both serve tasks to finish. A serve task that returned `Err`
        // already recorded itself via `record_serve_failure_named` inside the
        // task; but a task that PANICKED yields a `JoinError` here and recorded
        // nothing, so the failure slot would stay empty and the sibling listener
        // would serve on. Record the JoinError too and arm the failure flag so the
        // signal watcher winds the other listener down — consistent with
        // `run_plain_http`, which surfaces join errors rather than swallowing them.
        while let Some(joined) = tasks.join_next().await {
            if let Err(join_err) = joined {
                dux_core::logger::error(&format!(
                    "[server] an ACME serve task panicked: {join_err} — shutting the other \
                     listener down so the server does not limp on half-dead."
                ));
                record_serve_failure_named(
                    &serve_failed,
                    &serve_error,
                    anyhow::anyhow!("an ACME serve task panicked: {join_err}"),
                );
                http_handle.graceful_shutdown(Some(ACME_GRACEFUL_SHUTDOWN));
                https_handle.graceful_shutdown(Some(ACME_GRACEFUL_SHUTDOWN));
            }
        }

        // Wind down the ACME poller and the sweep.
        acme_task.abort();
        let _ = sweep_shutdown_tx.send(true);
        let _ = sweep.await;

        handle.shutdown().await;

        if let Ok(mut slot) = serve_error.lock()
            && let Some(e) = slot.take()
        {
            return Err(e);
        }
        Ok::<(), anyhow::Error>(())
    })
}

/// Bounded graceful-shutdown window for the ACME listeners: long enough for
/// in-flight requests to finish, short enough that a wedged TLS connection cannot
/// hang process exit.
const ACME_GRACEFUL_SHUTDOWN: Duration = Duration::from_secs(3);

/// Resolve once `flag` becomes `true`, polling on a short interval. Used by the
/// ACME signal watcher to react to a first-listener failure (the serve tasks set
/// the flag) without a dedicated channel.
async fn wait_for_flag(flag: Arc<AtomicBool>) {
    loop {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Record the first serve failure for the ACME path: the first caller stores its
/// error and arms the flag; later callers no-op the error slot. Always arms the
/// flag so the signal watcher triggers graceful shutdown. A trimmed sibling of
/// [`record_serve_failure`] (no shutdown watch — the ACME path uses axum-server
/// `Handle`s instead).
fn record_serve_failure_named(
    serve_failed: &AtomicBool,
    serve_error: &std::sync::Mutex<Option<anyhow::Error>>,
    err: anyhow::Error,
) {
    let first = serve_failed
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    if first && let Ok(mut slot) = serve_error.lock() {
        *slot = Some(err);
    }
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

/// Record a serve-task failure exactly once and ask the whole flip to wind down.
///
/// Called by every per-listener serve task whose accept loop returns an `Err`
/// (graceful shutdown returns `Ok`, so this only fires on a genuine death). The
/// FIRST caller wins: it stores the error and is the one that should arm the
/// engine-loop exit; later callers (other listeners winding down behind it) no-op
/// the error slot. Always trips the shutdown watch so the remaining listeners
/// stop too. Returns `true` when this call was the first-error winner.
fn record_serve_failure(
    serve_failed: &std::sync::atomic::AtomicBool,
    serve_error: &std::sync::Mutex<Option<anyhow::Error>>,
    shutdown_tx: &tokio::sync::watch::Sender<bool>,
    err: anyhow::Error,
) -> bool {
    let first = serve_failed
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    if first && let Ok(mut slot) = serve_error.lock() {
        *slot = Some(err);
    }
    // Always wind the others down, even for a non-first caller (idempotent send).
    let _ = shutdown_tx.send(true);
    first
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
    // down, so there is no rebind race); tokio needs them non-blocking.
    let tokio_listeners = {
        let _guard = runtime.enter();
        let mut out = Vec::with_capacity(listeners.len());
        for listener in listeners {
            listener.set_nonblocking(true)?;
            out.push(tokio::net::TcpListener::from_std(listener)?);
        }
        out
    };

    // Graceful-shutdown trigger for axum: the synchronous engine loop flips this
    // watch to `true` on exit, and each server's shutdown future awaits it.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    // Set by the signal task; polled by the control closure so a SIGINT/SIGTERM
    // received while serving breaks the engine loop too (not just axum).
    let signal_quit = Arc::new(AtomicBool::new(false));
    // Tripped by a serve task whose accept loop ERRORS while the server should
    // still be running. Without this, one listener's accept loop could die and
    // the remaining listeners would keep serving with no signal — the flip would
    // never return and the operator would see a silently half-dead server. The
    // control closure polls it (like `signal_quit`) so the engine loop exits, and
    // the captured error is returned so the flip surfaces the failure. This
    // mirrors `run_server`'s first-error-abort behavior.
    let serve_failed = Arc::new(AtomicBool::new(false));
    let serve_error: Arc<std::sync::Mutex<Option<anyhow::Error>>> =
        Arc::new(std::sync::Mutex::new(None));

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
        tls::spawn_session_sweep(sweep_store, SESSION_SWEEP_PERIOD, shutdown_rx.clone())
    };

    // One axum serve task per listener, all sharing the same router/state and the
    // same shutdown watch. A JoinSet lets us join them all (bounded) on teardown.
    let mut server_tasks = tokio::task::JoinSet::new();
    for tokio_listener in tokio_listeners {
        let app = app.clone();
        let mut shutdown_rx = shutdown_rx.clone();
        let task_shutdown_tx = shutdown_tx.clone();
        let task_serve_failed = Arc::clone(&serve_failed);
        let task_serve_error = Arc::clone(&serve_error);
        server_tasks.spawn_on(
            async move {
                let result = axum::serve(
                    tokio_listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(async move {
                    // Wait until the loop flips the watch to `true`.
                    while !*shutdown_rx.borrow_and_update() {
                        if shutdown_rx.changed().await.is_err() {
                            break;
                        }
                    }
                })
                .await;
                if let Err(err) = &result {
                    // The accept loop died while serving (graceful shutdown returns
                    // Ok). Log loudly, then record the first error and trip both the
                    // serve-failed flag (so the engine loop exits) and the shutdown
                    // watch (so the OTHER listeners wind down too) — never let the
                    // server limp on with one dead listener.
                    dux_core::logger::error(&format!(
                        "[server] a listener's accept loop failed; shutting the flip down: {err}"
                    ));
                    record_serve_failure(
                        &task_serve_failed,
                        &task_serve_error,
                        &task_shutdown_tx,
                        anyhow::anyhow!("web server listener failed: {err}"),
                    );
                }
                result
            },
            runtime.handle(),
        );
    }
    drop(shutdown_rx);
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
        if serve_failed.load(Ordering::SeqCst) {
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
    let _ = shutdown_tx.send(true);
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
    if let Ok(mut slot) = serve_error.lock()
        && let Some(err) = slot.take()
    {
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
    use super::{record_serve_failure, resolve_host_only};
    use dux_core::engine::Command;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn record_serve_failure_first_caller_wins_and_triggers_shutdown() {
        // The first serve task to die records its error, arms the flag, and trips
        // the shutdown watch; a later caller (another listener winding down) does
        // NOT overwrite the first error but STILL nudges shutdown. This is the F5
        // load-bearing logic, tested directly because forcing a real axum accept
        // loop to error mid-serve is inherently flaky.
        let failed = AtomicBool::new(false);
        let error: Mutex<Option<anyhow::Error>> = Mutex::new(None);
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let first = record_serve_failure(
            &failed,
            &error,
            &shutdown_tx,
            anyhow::anyhow!("listener A died"),
        );
        assert!(first, "the first failure must win");
        assert!(failed.load(Ordering::SeqCst), "the flag must be armed");
        assert!(
            *shutdown_rx.borrow_and_update(),
            "the shutdown watch must be tripped so other listeners wind down"
        );
        assert_eq!(
            error.lock().unwrap().as_ref().unwrap().to_string(),
            "listener A died"
        );

        // A second listener failing afterwards must NOT clobber the first error.
        let second = record_serve_failure(
            &failed,
            &error,
            &shutdown_tx,
            anyhow::anyhow!("listener B died"),
        );
        assert!(!second, "a later failure is not the first-error winner");
        assert_eq!(
            error.lock().unwrap().as_ref().unwrap().to_string(),
            "listener A died",
            "the first error is preserved"
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
