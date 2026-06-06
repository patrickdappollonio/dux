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
pub mod web_assets;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use dux_core::config::DuxPaths;
use dux_core::engine::Engine;

use crate::engine_actor::LoopControl;

/// Boot the engine on its own thread and serve the web UI on every address in
/// `addrs` (one axum task per listener, sharing the router/state). Blocking
/// entry — builds its own tokio runtime.
///
/// `all_addrs_local` is the caller's classification of whether EVERY bound
/// address is local (loopback OR the machine's own Tailscale address). It feeds
/// the auth-reload context's downgrade rule: the refuse-downgrade protection
/// guards PUBLIC binds, and a Tailscale bind is local (reachable only over the
/// operator's own WireGuard tailnet), so an all-local serve permits a reload
/// that turns the gate off — identical to a pure loopback bind. We thread the
/// classification rather than recomputing `addr.is_loopback()` per address
/// precisely so a Tailscale address does NOT count as a non-loopback public bind.
///
/// `disable_auth` mirrors the `dux server --disable-auth` flag: with it set the
/// login gate is off even when `[auth]` users exist. The gate's shared auth
/// snapshot is built ONCE here from the engine's loaded config users, handed to
/// both the engine actor (so a config reload rebuilds it live) and the router.
pub fn run_server(
    paths: DuxPaths,
    addrs: Vec<SocketAddr>,
    all_addrs_local: bool,
    disable_auth: bool,
) -> Result<()> {
    let engine = bootstrap::bootstrap_engine(&paths)?;
    // Build the login gate's shared state from the loaded config users (parsed
    // ONCE here, not per request). The same `Arc` is handed to the engine actor
    // (for live reload refresh) and to the router (for login/gate reads).
    let auth = auth::shared_auth(&engine.config.auth.users, disable_auth);
    let (handle, _join) = engine_actor::spawn_engine_thread_with_auth(
        engine,
        engine_actor::AuthReloadContext {
            shared: Arc::clone(&auth),
            disable_auth,
            loopback: all_addrs_local,
        },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        // Bind every address; share one router across all listeners by cloning
        // the axum app (it is a cheap `Arc`-backed service). Each serve task gets
        // its own graceful-shutdown future driven by the same signal.
        let mut tasks = tokio::task::JoinSet::new();
        for addr in addrs {
            let listener = tokio::net::TcpListener::bind(addr).await?;
            let app = server::router_with_auth(handle.clone(), Arc::clone(&auth));
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
        while let Some(joined) = tasks.join_next().await {
            joined??;
        }
        // SIGTERM the agents (they save state for a later resume), mark their
        // sessions Detached, then exit; Drop hard-kills any straggler.
        handle.shutdown().await;
        Ok::<(), anyhow::Error>(())
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
    // The flip is ALWAYS local by construction: `local_addrs` only ever yields
    // loopback plus the machine's own Tailscale address, never a public bind. A
    // Tailscale bind is reachable only over the operator's own WireGuard tailnet,
    // so it is local-classified — the auth-reload downgrade rule (which protects
    // PUBLIC binds) treats this serve exactly like a pure loopback bind, allowing
    // a reload that removes the last user. We therefore pass `loopback: true`
    // unconditionally rather than recomputing from per-listener `is_loopback()`,
    // which would wrongly flag the Tailscale listener as non-loopback.
    let (handle, ends) = engine_actor::build_actor_channels_with_auth(
        &engine,
        Some(engine_actor::AuthReloadContext {
            shared: Arc::clone(&auth),
            disable_auth: false,
            loopback: true,
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

    // One axum serve task per listener, all sharing the same router/state and the
    // same shutdown watch. A JoinSet lets us join them all (bounded) on teardown.
    let mut server_tasks = tokio::task::JoinSet::new();
    for tokio_listener in tokio_listeners {
        let app = server::router_with_auth(handle.clone(), Arc::clone(&auth));
        let mut shutdown_rx = shutdown_rx.clone();
        server_tasks.spawn_on(
            async move {
                axum::serve(
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
                .await
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
    // exit reason: a tripped signal flag wins (QuitProcess), otherwise the
    // caller's tick result maps straight through.
    let mut exit = ServerExit::ReturnToTui;
    let mut engine = engine_actor::run_engine_loop(engine, ends, || {
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
    let _ = shutdown_tx.send(true);
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
    use dux_core::engine::Command;

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
